use crate::circuit::{derive_server_key, evaluate_circuit, get_cells, PARAMETER};
use crate::dashboard::{Dashboard, RegisteredUser};

use crate::types::{
    CircuitOutput, DecryptionShare, DecryptionShareSubmission, EncryptedWord, Error, ErrorResponse,
    GameStateEnc, MutexServerStorage, Seed, ServerState, ServerStorage, SksSubmission, UserId,
    UserStorage,
};
use crate::{AnnotatedDecryptionShare, UserAction, Word};
use phantom_zone::{set_common_reference_seed, set_parameter_set};
use rand::{thread_rng, RngCore};
use rocket::serde::json::Json;
use rocket::serde::msgpack::MsgPack;
use rocket::{get, post, routes};
use rocket::{Build, Rocket, State};
use tokio::sync::Mutex;

#[get("/param")]
async fn get_param(ss: &State<MutexServerStorage>) -> Json<Seed> {
    let ss = ss.lock().await;
    Json(ss.seed)
}

/// A user registers a name and get an ID
/// We support 4 players
#[post("/register", data = "<name>")]
async fn register(
    name: &str,
    ss: &State<MutexServerStorage>,
) -> Result<Json<RegisteredUser>, ErrorResponse> {
    let mut ss = ss.lock().await;
    ss.ensure(ServerState::ReadyForJoining)?;
    let user = ss.add_user(name);
    println!("{name} just joined!");

    if ss.users.len() == 4 {
        ss.ensure(ServerState::ReadyForJoining)?;
        ss.transit(ServerState::ReadyForInputs);
        println!("Got 4 players. Registration closed!");
    }

    Ok(Json(user))
}

#[get("/dashboard")]
async fn get_dashboard(ss: &State<MutexServerStorage>) -> Json<Dashboard> {
    let dashboard = ss.lock().await.get_dashboard();
    Json(dashboard)
}

/// The user submits Server key shares
#[post("/submit_sks", data = "<submission>", format = "msgpack")]
async fn submit_sks(
    submission: MsgPack<SksSubmission>,
    ss: &State<MutexServerStorage>,
) -> Result<Json<UserId>, ErrorResponse> {
    let mut ss = ss.lock().await;

    ss.ensure(ServerState::ReadyForInputs)?;

    let SksSubmission { user_id, sks } = submission.0;

    let user = ss.get_user(user_id)?;
    println!("{} submited data", user.name);
    user.storage = UserStorage::Sks(Box::new(sks));

    if ss.check_cipher_submission() {
        ss.transit(ServerState::ReadyForRunning);
    }

    Ok(Json(user_id))
}

#[post("/request_action/<user_id>", data = "<action>", format = "msgpack")]
async fn request_action(
    user_id: UserId,
    action: MsgPack<UserAction<EncryptedWord>>,
    ss: &State<MutexServerStorage>,
) -> Result<Json<UserId>, ErrorResponse> {
    let mut ss = ss.lock().await;

    ss.ensure(ServerState::ReadyForInputs)?;
    let user = ss.get_user(user_id)?;
    println!("{} performed {}", user.name, action.to_string());
    let action = action.unpack(user_id);
    match action {
        UserAction::InitGame { initial_eggs } => {
            match &mut ss.game_state {
                Some(game_state) => game_state.eggs = initial_eggs,
                None => {
                    ss.game_state = Some(GameStateEnc {
                        coords: vec![],
                        eggs: initial_eggs,
                    })
                }
            };
        }
        UserAction::SetStartingCoords { starting_coords } => {
            match &mut ss.game_state {
                Some(game_state) => game_state.coords = starting_coords,
                None => {
                    ss.game_state = Some(GameStateEnc {
                        coords: starting_coords,
                        eggs: vec![],
                    })
                }
            };
        }
        UserAction::MovePlayer { .. }
        | UserAction::LayEgg { .. }
        | UserAction::PickupEgg { .. }
        | UserAction::GetCell { .. } => ss.action_queue.push((user_id, action)),
    };

    Ok(Json(user_id))
}

/// The admin runs the fhe computation
#[post("/run")]
async fn run(ss: &State<MutexServerStorage>) -> Result<Json<ServerState>, ErrorResponse> {
    let s2 = (*ss).clone();
    let mut ss = ss.lock().await;

    match &ss.state {
        ServerState::ReadyForRunning => {
            let server_key_shares = ss.get_sks()?;
            let game_state = ss.game_state.clone().ok_or(Error::GameNotInitedYet)?;
            let uas = ss.action_queue.clone();

            tokio::task::spawn_blocking(move || {
                rayon::ThreadPoolBuilder::new()
                    .build_scoped(
                        // Initialize thread-local storage parameters
                        |thread| {
                            set_parameter_set(PARAMETER);
                            thread.run()
                        },
                        // Run parallel code under this pool
                        |pool| {
                            pool.install(|| {
                                println!("Begin FHE run");
                                // Long running, global variable change
                                derive_server_key(&server_key_shares);

                                // Long running
                                let final_game_state = evaluate_circuit(game_state, &uas);
                                let cells = get_cells(&final_game_state, 4);
                                let mut ss = s2.blocking_lock();
                                ss.game_state = Some(final_game_state);
                                let cells = CircuitOutput::new(cells);
                                ss.cells = Some(cells);
                                ss.transit(ServerState::CompletedFhe);
                                println!("FHE computation completed");
                            })
                        },
                    )
                    .unwrap();
            });
            ss.transit(ServerState::RunningFhe);
            Ok(Json(ServerState::RunningFhe))
        }
        ServerState::RunningFhe => Ok(Json(ServerState::RunningFhe)),
        ServerState::CompletedFhe => Ok(Json(ServerState::CompletedFhe)),
        _ => Err(Error::WrongServerState {
            expect: ServerState::ReadyForRunning.to_string(),
            got: ss.state.to_string(),
        }
        .into()),
    }
}

#[get("/fhe_output")]
async fn get_fhe_output(
    ss: &State<MutexServerStorage>,
) -> Result<Json<CircuitOutput>, ErrorResponse> {
    let ss = ss.lock().await;
    ss.ensure(ServerState::CompletedFhe)?;
    let cells = ss.cells.clone().ok_or(Error::CellNotFound)?;
    Ok(Json(cells))
}

/// The user submits the ciphertext
#[post("/submit_decryption_shares", data = "<submission>", format = "msgpack")]
async fn submit_decryption_shares(
    submission: MsgPack<DecryptionShareSubmission>,
    ss: &State<MutexServerStorage>,
) -> Result<Json<UserId>, ErrorResponse> {
    let user_id = submission.user_id;
    let mut ss = ss.lock().await;
    let decryption_shares = ss
        .get_user(user_id)?
        .storage
        .get_mut_decryption_shares()
        .ok_or(Error::OutputNotReady)?;
    *decryption_shares = Some(submission.decryption_shares.to_vec());
    Ok(Json(user_id))
}

#[get("/decryption_share/<fhe_output_id>/<user_id>")]
async fn get_decryption_share(
    fhe_output_id: usize,
    user_id: UserId,
    ss: &State<MutexServerStorage>,
) -> Result<Json<AnnotatedDecryptionShare>, ErrorResponse> {
    let mut ss: tokio::sync::MutexGuard<ServerStorage> = ss.lock().await;
    let decryption_shares = ss
        .get_user(user_id)?
        .storage
        .get_mut_decryption_shares()
        .cloned()
        .ok_or(Error::OutputNotReady)?
        .ok_or(Error::DecryptionShareNotFound {
            output_id: fhe_output_id,
            user_id,
        })?;
    Ok(Json(decryption_shares[fhe_output_id].clone()))
}

pub fn setup(seed: &Seed) {
    set_parameter_set(PARAMETER);
    set_common_reference_seed(*seed);
}

pub fn rocket() -> Rocket<Build> {
    let mut seed = [0u8; 32];
    thread_rng().fill_bytes(&mut seed);
    setup(&seed);

    rocket::build()
        .manage(MutexServerStorage::new(Mutex::new(ServerStorage::new(
            seed,
        ))))
        .mount(
            "/",
            routes![
                get_param,
                register,
                get_dashboard,
                submit_sks,
                request_action,
                run,
                get_fhe_output,
                submit_decryption_shares,
                get_decryption_share,
            ],
        )
}
