use crate::circuit::{derive_server_key, evaluate_circuit, get_user_cell, PARAMETER};
use crate::dashboard::{Dashboard, RegisteredUser};

use crate::types::{
    CircuitOutput, DecryptionShare, DecryptionShareSubmission, EncryptedWord, Error, ErrorResponse,
    GameStateEnc, MutexServerStorage, Seed, ServerState, ServerStorage, SksSubmission, UserId,
    UserStorage,
};
use crate::UserAction;
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
        ss.transit(ServerState::ReadyForServerKeyShares);
        println!("Got 4 players. Registration closed!");
    }

    Ok(Json(user))
}

#[get("/dashboard")]
async fn get_dashboard(ss: &State<MutexServerStorage>) -> Json<Dashboard> {
    let dashboard = ss.lock().await.get_dashboard();
    Json(dashboard)
}

/// The user submits server key shares
#[post("/submit_sks", data = "<submission>", format = "msgpack")]
async fn submit_sks(
    submission: MsgPack<SksSubmission>,
    ss: &State<MutexServerStorage>,
) -> Result<Json<UserId>, ErrorResponse> {
    let mut ss = ss.lock().await;

    ss.ensure(ServerState::ReadyForServerKeyShares)?;

    let SksSubmission { user_id, sks } = submission.0;

    let user = ss.get_user(user_id)?;
    println!("{} submited server key share.", user.name);
    user.storage = UserStorage::Sks(Box::new(sks));

    if ss.check_cipher_submission() {
        ss.transit(ServerState::ReadyForSetupGame);
        let server_key_shares = ss.get_sks()?;
        set_parameter_set(PARAMETER);
        // Long running, global variable change
        derive_server_key(&server_key_shares);
    }

    Ok(Json(user_id))
}

#[post("/setup_game/<user_id>", data = "<action>", format = "msgpack")]
async fn setup_game(
    user_id: UserId,
    action: MsgPack<UserAction<EncryptedWord>>,
    ss: &State<MutexServerStorage>,
) -> Result<Json<UserId>, ErrorResponse> {
    let mut ss = ss.lock().await;

    ss.ensure(ServerState::ReadyForSetupGame)?;

    let user = ss.get_user(user_id)?;
    println!("{} requested action {}", user.name, action.to_string());
    let action = action.unpack(user_id);

    let result = match action {
        UserAction::InitGame { initial_eggs } => {
            match &mut ss.game_state {
                Some(game_state) => game_state.eggs = initial_eggs,
                None => {
                    ss.game_state = Some(GameStateEnc {
                        coords: vec![None; 4],
                        eggs: initial_eggs,
                    })
                }
            };
            Ok(Json(user_id))
        }
        UserAction::SetStartingCoord { starting_coord } => {
            user.storage = UserStorage::StartingCoords;
            match &mut ss.game_state {
                Some(game_state) => game_state.coords[user_id] = Some(starting_coord),
                None => {
                    let mut coords = vec![None; 4];
                    coords[user_id] = Some(starting_coord);
                    ss.game_state = Some(GameStateEnc {
                        coords,
                        eggs: vec![],
                    });
                }
            };
            if ss.check_setup_game_complete() {
                ss.transit(ServerState::ReadyForActions);
                for user in ss.users.iter_mut() {
                    user.storage = UserStorage::DecryptionShare(None);
                }
            }
            Ok(Json(user_id))
        }
        _ => Err(Error::WrongServerState {
            expect: ServerState::ReadyForSetupGame.to_string(),
            got: ss.state.to_string(),
        }
        .into()),
    };

    result
}

#[post("/request_action/<user_id>", data = "<action>", format = "msgpack")]
async fn request_action(
    user_id: UserId,
    action: MsgPack<UserAction<EncryptedWord>>,
    ss: &State<MutexServerStorage>,
) -> Result<Json<UserId>, ErrorResponse> {
    let mut ss = ss.lock().await;

    ss.ensure(ServerState::ReadyForActions)?;

    let user = ss.get_user(user_id)?;
    println!("{} requested action {}", user.name, action.to_string());
    let action = action.unpack(user_id);

    let result = match action {
        UserAction::MovePlayer { .. }
        | UserAction::LayEgg { .. }
        | UserAction::PickupEgg { .. }
        | UserAction::GetCell { .. } => {
            ss.action_queue.push((user_id, action));
            ss.transit(ServerState::ReadyForRunning);
            Ok(Json(user_id))
        }
        _ => Err(Error::WrongServerState {
            expect: ServerState::ReadyForActions.to_string(),
            got: ss.state.to_string(),
        }
        .into()),
    };

    result
}

#[post("/done/<user_id>", data = "<action>", format = "msgpack")]
async fn done(
    user_id: UserId,
    action: MsgPack<UserAction<EncryptedWord>>,
    ss: &State<MutexServerStorage>,
) -> Result<Json<UserId>, ErrorResponse> {
    let mut ss = ss.lock().await;

    ss.ensure(ServerState::CompletedFhe)?;

    let user = ss.get_user(user_id)?;
    println!("{} requested action {}", user.name, action.to_string());
    let action = action.unpack(user_id);

    let result = match action {
        UserAction::Done => {
            user.ready_for_new_round = true;
            Ok(Json(user_id))
        }
        _ => Err(Error::WrongServerState {
            expect: ServerState::CompletedFhe.to_string(),
            got: ss.state.to_string(),
        }
        .into()),
    };

    if ss.check_ready_for_new_round() {
        ss.transit(ServerState::ReadyForActions);
        for user in ss.users.iter_mut() {
            user.ready_for_new_round = false;
            user.storage = UserStorage::DecryptionShare(None);
        }
    }

    result
}

#[post("/run/<user_id>")]
async fn run(
    user_id: UserId,
    ss: &State<MutexServerStorage>,
) -> Result<Json<ServerState>, ErrorResponse> {
    let s2 = (*ss).clone();
    let mut ss = ss.lock().await;

    match &ss.state {
        ServerState::ReadyForRunning => {
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
                                // Long running
                                let final_game_state = evaluate_circuit(game_state, &uas);

                                let cell = get_user_cell(&final_game_state, user_id);
                                let mut ss = s2.blocking_lock();
                                ss.game_state = Some(final_game_state);
                                let cell = CircuitOutput::new(cell);
                                ss.circuit_output = Some(cell);

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
    let cell = ss.circuit_output.clone().ok_or(Error::CellNotFound)?;
    Ok(Json(cell))
}

/// The user submits the ciphertext
#[post("/submit_decryption_share", data = "<submission>", format = "msgpack")]
async fn submit_decryption_share(
    submission: MsgPack<DecryptionShareSubmission>,
    ss: &State<MutexServerStorage>,
) -> Result<Json<UserId>, ErrorResponse> {
    let user_id = submission.user_id;
    let mut ss = ss.lock().await;
    ss.ensure(ServerState::CompletedFhe)?;
    let decryption_share = ss
        .get_user(user_id)?
        .storage
        .get_mut_decryption_share()
        .ok_or(Error::OutputNotReady)?;
    *decryption_share = Some(submission.decryption_share.clone());
    Ok(Json(user_id))
}

#[get("/decryption_share/<user_id>")]
async fn get_decryption_share(
    user_id: UserId,
    ss: &State<MutexServerStorage>,
) -> Result<Json<DecryptionShare>, ErrorResponse> {
    let mut ss: tokio::sync::MutexGuard<ServerStorage> = ss.lock().await;
    ss.ensure(ServerState::CompletedFhe)?;
    let decryption_share = ss
        .get_user(user_id)?
        .storage
        .get_mut_decryption_share()
        .cloned()
        .ok_or(Error::OutputNotReady)?
        .ok_or(Error::DecryptionShareNotFound { user_id })?;
    Ok(Json(decryption_share.clone()))
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
                setup_game,
                request_action,
                done,
                run,
                get_fhe_output,
                submit_decryption_share,
                get_decryption_share,
            ],
        )
}
