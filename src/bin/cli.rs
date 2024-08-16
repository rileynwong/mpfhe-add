use anyhow::{anyhow, bail, Error};
use chickens::{
    setup, CircuitOutput, DecryptionSharesMap, Direction, GameStateLocalView, ServerState, UserId,
    WebClient, BOARD_SIZE,
};
use clap::{command, Parser};
use itertools::Itertools;
use phantom_zone::{gen_client_key, gen_server_key_share, ClientKey};
use rustyline::{error::ReadlineError, DefaultEditor};
use std::{collections::HashMap, fmt::Display};

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Cli2 {
    /// Optional name to operate on
    name: String,
    url: String,
}

enum State {
    Init(StateInit),
    Setup(StateSetup),
    ConcludedRegistration(Registration),
    SubmittedSks(Registration),
    ConcludedSubmitSks(Registration),
    InitGame(StateGame),
    SetupGame(StateGame), // set starting coordinates
    ConcludedSetupGame(StateGame),
    GameAction(StateGameAction),
    CompletedFhe(StateGameAction),
    DownloadedOutput(StateDownloadedOutput),
    ConcludedDecryptionSubmission(StateDownloadedOutput),
    Decrypted(StateDecrypted),
}

impl Display for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            State::Init(_) => "Initialization",
            State::Setup(_) => "Setup",
            State::ConcludedRegistration(_) => "Concluded Registration",
            State::SubmittedSks(_) => "Submitted Server Key Share",
            State::ConcludedSubmitSks(_) => "Concluded SubmitSks",
            State::InitGame(_) => "Init game",
            State::SetupGame(_) => "Setup game",
            State::ConcludedSetupGame(_) => "Concluded setup game",
            State::GameAction(_) => "A player took an action",
            State::CompletedFhe(_) => "Completed FHE",
            State::DownloadedOutput(_) => "Downloaded Output",
            State::ConcludedDecryptionSubmission(_) => "Concluded decryption share submission",
            State::Decrypted(_) => "Decrypted",
        };
        write!(f, "{{{{ {} }}}}", label)
    }
}

impl State {
    fn print_status_update(&self) {
        let msg = match self {
            State::Init(StateInit { name, client }) => {
                format!("Hi {}, we just connected to server {}.", name, client.url())
            }
            State::Setup(StateSetup { .. }) => "✅ Setup completed!".to_string(),
            State::ConcludedRegistration(_) => "✅ Got 4 players!".to_string(),
            State::SubmittedSks(_) => "✅ Server key share submitted!".to_string(),
            State::ConcludedSubmitSks(_) => "✅ Got all 4 server key shares!".to_string(),
            State::InitGame(_) => "✅ New game start!".to_string(),
            State::SetupGame(_) => "✅ Set starting coordinates!".to_string(),
            State::ConcludedSetupGame(_) => "✅ Got all 4 starting coordinates!".to_string(),
            State::GameAction(_) => "✅ A player took an action!".to_string(),
            State::CompletedFhe(_) => "✅ Completed FHE!".to_string(),
            State::DownloadedOutput(_) => "✅ FHE output downloaded!".to_string(),
            State::ConcludedDecryptionSubmission(_) => {
                "✅ Decryption shares submitted!".to_string()
            }
            State::Decrypted(_) => "✅ FHE output decrypted!".to_string(),
        };
        println!("{}", msg)
    }

    fn print_instruction(&self) {
        let msg = match self {
            State::Setup(_) => "We need 4 players. Enter `next` to check if we can proceed.",
            State::SubmittedSks(_) =>
                "Server needs to get all 4 server key shares. Enter `next` to check if we can proceed.",
            State::ConcludedSubmitSks(_) => "Enter `next` to start a new game.",
            State::InitGame(_) => "Enter `next ${x} ${y}` with your starting coordinates (x, y).\n The board is 4 x 4, so x, y has to be in the range [0, 3]." ,
            State::SetupGame(_) => "Wait for every user to set starting coordinates. Enter `next` to check if we can proceed.",
            State::ConcludedSetupGame(_) => "Enter one of the commands {`move up` | `move down` | `move left` | `move right` | `lay` | `pickup`}",
            State::GameAction(_) => "Server running FHE. Enter `next` to check if it completed",
            State::DownloadedOutput(_) => "Wait for other players to submit decryption shares. Enter `next` to check if we can proceed.",
            State::Decrypted(_) => "Exit with `CTRL-D`",
            _ => "Enter `next` to continue",
        };
        println!("👇 {}", msg)
    }
}

struct StateInit {
    name: String,
    client: WebClient,
}

struct StateSetup {
    name: String,
    client: WebClient,
    ck: ClientKey,
    user_id: UserId,
}

struct Registration {
    name: String,
    client: WebClient,
    ck: ClientKey,
    user_id: UserId,
    names: Vec<String>,
}

struct StateGame {
    name: String,
    client: WebClient,
    ck: ClientKey,
    user_id: UserId,
    names: Vec<String>,
    view: GameStateLocalView,
}

struct StateGameAction {
    name: String,
    client: WebClient,
    ck: ClientKey,
    user_id: UserId,
    names: Vec<String>,
    view: GameStateLocalView,
    is_my_action: bool, // whether I took the action, or other players took the action
}

struct StateDownloadedOutput {
    #[allow(dead_code)]
    name: String,
    client: WebClient,
    ck: ClientKey,
    user_id: UserId,
    names: Vec<String>,
    fhe_out: CircuitOutput,
    shares: DecryptionSharesMap,
    view: GameStateLocalView,
    is_my_action: bool, // only decrypt if it is my action
}

struct StateDecrypted {
    #[allow(dead_code)]
    names: Vec<String>,
    client: WebClient,
    decrypted_output: Option<Vec<bool>>,
    view: GameStateLocalView,
}

#[tokio::main]
async fn main() {
    let cli = Cli2::parse();
    let name = cli.name;
    let url: String = cli.url;

    let mut rl = DefaultEditor::new().unwrap();
    let client = WebClient::new(&url);
    let mut state = State::Init(StateInit { name, client });
    println!("{}", state);
    state.print_status_update();
    state.print_instruction();
    loop {
        let readline = rl.readline(">> ");
        match readline {
            Ok(line) => {
                rl.add_history_entry(line.as_str()).unwrap();
                state = match run(state, line.as_str()).await {
                    Ok(state) => {
                        println!("{}", state);
                        state.print_status_update();
                        state
                    }
                    Err((err, state)) => {
                        println!("❌ Error: {:?}", err);
                        println!("Fallback to {}", state);
                        state
                    }
                };
                state.print_instruction();
            }
            Err(ReadlineError::Interrupted) => {
                println!("CTRL-C");
                break;
            }
            Err(ReadlineError::Eof) => {
                println!("CTRL-D");
                break;
            }
            Err(err) => {
                println!("Error: {:?}", err);
                break;
            }
        }
    }
}

async fn cmd_setup(name: &str, client: &WebClient) -> Result<(ClientKey, usize), Error> {
    let seed = client.get_seed().await?;
    println!(
        "Acquired seed for commen reference string (CRS) 0x{}",
        hex::encode(seed)
    );
    println!("Setup my CRS");
    setup(&seed);
    println!("Generate my client key");
    let ck = gen_client_key();
    let user = client.register(name).await?;
    println!("Hi {}, you are registered with ID: {}", user.name, user.id);
    Ok((ck, user.id))
}

async fn cmd_get_names(client: &WebClient) -> Result<(bool, Vec<String>), Error> {
    let d = client.get_dashboard().await?;
    d.print_presentation();
    Ok((d.is_concluded(), d.get_names()))
}

async fn cmd_submit_sks(client: &WebClient, ck: &ClientKey, user_id: &UserId) -> Result<(), Error> {
    let total_users = 4;
    println!("Generating server key share");
    let sks = gen_server_key_share(*user_id, total_users, ck);
    println!("Submit server key share");
    client.submit_sks(*user_id, &sks).await?;
    Ok(())
}

async fn cmd_check_submit_sks_complete(client: &WebClient) -> Result<bool, Error> {
    let d = client.get_dashboard().await?;
    d.print_presentation();
    Ok(d.is_submit_sks_complete())
}

async fn cmd_init_game(client: &WebClient, ck: &ClientKey, user_id: UserId) -> Result<(), Error> {
    let initial_eggs = [false; BOARD_SIZE];
    client.init_game(ck, user_id, &initial_eggs).await?;
    Ok(())
}

async fn cmd_setup_game(
    args: &[&str],
    client: &WebClient,
    ck: &ClientKey,
    user_id: UserId,
) -> Result<GameStateLocalView, Error> {
    let x = args
        .get(0)
        .ok_or_else(|| anyhow!("please add init x coordinate"))?
        .parse::<u8>()?;
    let y = args
        .get(1)
        .ok_or_else(|| anyhow!("please add init y coordinate"))?
        .parse::<u8>()?;

    if x > 3 {
        return Err(anyhow!("init x coordinate has to be in the range [0, 3]"));
    }
    if y > 3 {
        return Err(anyhow!("init y coordinate has to be in the range [0, 3]"));
    }

    let view = GameStateLocalView::new(x, y, user_id);
    client.set_starting_coords(ck, user_id, &(x, y)).await?;
    view.print();
    Ok(view)
}

async fn cmd_setup_game_complete(client: &WebClient) -> Result<bool, Error> {
    let d = client.get_dashboard().await?;
    d.print_presentation();
    Ok(d.is_setup_game_complete())
}

async fn cmd_move(
    args: &[&str],
    client: &WebClient,
    ck: &ClientKey,
    user_id: UserId,
    view: &GameStateLocalView,
) -> Result<GameStateLocalView, Error> {
    let arg = args
        .get(0)
        .ok_or_else(|| anyhow!("please add direction to move"))?;
    let direction = match *arg {
        "up" => Direction::Up,
        "down" => Direction::Down,
        "left" => Direction::Left,
        "right" => Direction::Right,
        &_ => bail!("invalid commmit"),
    };

    client.move_player(ck, user_id, direction).await?;
    let mut view = view.clone();
    view.move_player(direction);
    view.print();
    trigger_run(client, user_id).await?;
    Ok(view)
}

async fn cmd_lay(
    client: &WebClient,
    user_id: UserId,
    view: &GameStateLocalView,
) -> Result<GameStateLocalView, Error> {
    client.lay_egg(user_id).await?;
    let mut view = view.clone();
    view.lay();
    view.print();
    trigger_run(client, user_id).await?;
    Ok(view)
}

async fn cmd_pickup(
    client: &WebClient,
    user_id: UserId,
    view: &GameStateLocalView,
) -> Result<GameStateLocalView, Error> {
    client.pickup_egg(user_id).await?;
    let mut view = view.clone();
    view.pickup();
    view.print();
    trigger_run(client, user_id).await?;
    Ok(view)
}

async fn trigger_run(client: &WebClient, user_id: UserId) -> Result<(), Error> {
    println!("Requesting FHE run ...");
    let resp = client.trigger_fhe_run(user_id).await?;
    println!("Server: {}", resp);
    Ok(())
}

async fn cmd_fhe_complete(client: &WebClient) -> Result<bool, Error> {
    let d = client.get_dashboard().await?;
    Ok(d.is_fhe_complete())
}

async fn cmd_fhe_ongoing(client: &WebClient) -> Result<bool, Error> {
    let d = client.get_dashboard().await?;
    Ok(d.is_fhe_ongoing())
}

async fn cmd_download_output(
    client: &WebClient,
    user_id: &UserId,
    ck: &ClientKey,
    should_submit_shares: bool,
) -> Result<(CircuitOutput, DecryptionSharesMap), Error> {
    let resp = client.trigger_fhe_run(*user_id).await?;
    if !matches!(resp, ServerState::CompletedFhe) {
        bail!("FHE is still running")
    }

    println!("Downloading fhe output");
    let fhe_out = client.get_fhe_output().await?;

    println!("Generating my decrypting shares");
    let mut shares = HashMap::new();
    let my_decryption_share = fhe_out.gen_decryption_share(ck);
    shares.insert(*user_id, my_decryption_share.clone());

    if should_submit_shares {
        println!("Submitting my decrypting shares");
        client
            .submit_decryption_share(*user_id, &my_decryption_share)
            .await?;
    }

    Ok((fhe_out, shares))
}

async fn cmd_decryption_submission_completed(
    client: &WebClient,
    user_id: &UserId,
) -> Result<bool, Error> {
    let d = client.get_dashboard().await?;
    d.print_presentation();
    Ok(d.is_decryption_shares_submission_complete(*user_id))
}

async fn cmd_download_shares(
    client: &WebClient,
    names: &[String],
    ck: &ClientKey,
    shares: &mut DecryptionSharesMap,
    co: &CircuitOutput,
    view: &GameStateLocalView,
) -> Result<Vec<bool>, Error> {
    let total_users = names.len();
    println!("Acquiring decryption shares needed");
    for user_id in 0..total_users {
        if shares.get(&user_id).is_none() {
            let ds = client.get_decryption_share(user_id).await?;
            shares.insert(user_id, ds);
        }
    }
    println!("Decrypt the encrypted output");
    let dss = (0..total_users)
        .map(|user_id| shares.get(&user_id).expect("exists").to_owned())
        .collect_vec();
    let decrypted_output = co.decrypt(ck, &dss);
    println!("Final decrypted output: {:?}", decrypted_output);
    view.print_with_output(&decrypted_output);
    Ok(decrypted_output)
}

async fn run(state: State, line: &str) -> Result<State, (Error, State)> {
    let terms: Vec<&str> = line.split_whitespace().collect();
    if terms.is_empty() {
        return Ok(state);
    }
    let cmd = &terms[0];
    let args = &terms[1..];
    if cmd == &"next" {
        match state {
            State::Init(s) => match cmd_setup(&s.name, &s.client).await {
                Ok((ck, user_id)) => Ok(State::Setup(StateSetup {
                    name: s.name,
                    client: s.client,
                    ck,
                    user_id,
                })),
                Err(err) => Err((err, State::Init(s))),
            },
            State::Setup(s) => match cmd_get_names(&s.client).await {
                Ok((is_concluded, names)) => {
                    if is_concluded {
                        Ok(State::ConcludedRegistration(Registration {
                            name: s.name,
                            client: s.client,
                            ck: s.ck,
                            user_id: s.user_id,
                            names,
                        }))
                    } else {
                        Ok(State::Setup(s))
                    }
                }
                Err(err) => Err((err, State::Setup(s))),
            },
            State::ConcludedRegistration(s) => {
                match cmd_submit_sks(&s.client, &s.ck, &s.user_id).await {
                    Ok(()) => Ok(State::SubmittedSks(s)),
                    Err(err) => Err((err, State::ConcludedRegistration(s))),
                }
            }
            State::SubmittedSks(s) => match cmd_check_submit_sks_complete(&s.client).await {
                Ok(is_complete) => {
                    if is_complete {
                        Ok(State::ConcludedSubmitSks(s))
                    } else {
                        Ok(State::SubmittedSks(s))
                    }
                }
                Err(err) => Err((err, State::SubmittedSks(s))),
            },
            State::ConcludedSubmitSks(s) => {
                match cmd_init_game(&s.client, &s.ck, s.user_id).await {
                    Ok(()) => Ok(State::InitGame(StateGame {
                        name: s.name,
                        client: s.client,
                        ck: s.ck,
                        user_id: s.user_id,
                        names: s.names,
                        view: GameStateLocalView::new(0, 0, s.user_id),
                    })),
                    Err(err) => Err((err, State::ConcludedSubmitSks(s))),
                }
            }
            State::InitGame(s) => match cmd_setup_game(args, &s.client, &s.ck, s.user_id).await {
                Ok(view) => Ok(State::SetupGame(StateGame { view, ..s })),
                Err(err) => Err((err, State::InitGame(s))),
            },
            State::SetupGame(s) => match cmd_setup_game_complete(&s.client).await {
                Ok(is_complete) => {
                    if is_complete {
                        Ok(State::ConcludedSetupGame(s))
                    } else {
                        Ok(State::SetupGame(s))
                    }
                }
                Err(err) => Err((err, State::SetupGame(s))),
            },
            State::ConcludedSetupGame(s) => Ok(State::CompletedFhe(StateGameAction {
                is_my_action: false,
                name: s.name,
                client: s.client,
                ck: s.ck,
                user_id: s.user_id,
                names: s.names,
                view: s.view,
            })),
            State::GameAction(s) => match cmd_fhe_complete(&s.client).await {
                Ok(is_complete) => {
                    if is_complete {
                        Ok(State::CompletedFhe(s))
                    } else {
                        Ok(State::GameAction(s))
                    }
                }
                Err(err) => Err((err, State::GameAction(s))),
            },
            State::CompletedFhe(s) => {
                match cmd_download_output(&s.client, &s.user_id, &s.ck, !s.is_my_action).await {
                    Ok((fhe_out, shares)) => Ok(State::DownloadedOutput(StateDownloadedOutput {
                        name: s.name,
                        client: s.client,
                        ck: s.ck,
                        user_id: s.user_id,
                        names: s.names,
                        fhe_out,
                        shares,
                        view: s.view,
                        is_my_action: s.is_my_action,
                    })),
                    Err(err) => Err((err, State::CompletedFhe(s))),
                }
            }
            State::DownloadedOutput(s) => {
                if !s.is_my_action {
                    return Ok(State::Decrypted(StateDecrypted {
                        names: s.names,
                        client: s.client,
                        decrypted_output: None,
                        view: s.view,
                    }));
                }
                match cmd_decryption_submission_completed(&s.client, &s.user_id).await {
                    Ok(is_complete) => {
                        if is_complete {
                            Ok(State::ConcludedDecryptionSubmission(s))
                        } else {
                            Ok(State::DownloadedOutput(s))
                        }
                    }
                    Err(err) => Err((err, State::DownloadedOutput(s))),
                }
            }

            State::ConcludedDecryptionSubmission(mut s) => {
                match cmd_download_shares(
                    &s.client,
                    &s.names,
                    &s.ck,
                    &mut s.shares,
                    &s.fhe_out,
                    &s.view,
                )
                .await
                {
                    Ok(decrypted_output) => Ok(State::Decrypted(StateDecrypted {
                        names: s.names,
                        client: s.client,
                        decrypted_output: Some(decrypted_output),
                        view: s.view,
                    })),
                    Err(err) => Err((err, State::DownloadedOutput(s))),
                }
            }
            State::Decrypted(s) => Ok(State::Decrypted(s)),
        }
    } else if cmd == &"move" {
        match state {
            State::ConcludedSetupGame(s) => {
                match cmd_move(args, &s.client, &s.ck, s.user_id, &s.view).await {
                    Ok(view) => Ok(State::GameAction(StateGameAction {
                        view,
                        is_my_action: true,
                        name: s.name,
                        client: s.client,
                        ck: s.ck,
                        user_id: s.user_id,
                        names: s.names,
                    })),
                    Err(err) => {
                        if err.to_string().contains("Wrong server state") {
                            let result = match cmd_fhe_ongoing(&s.client).await {
                                Ok(is_ongoing) => {
                                    if is_ongoing {
                                        println!("❌ Your action DID NOT take effect!");
                                        println!("❗ Another player took an action first. Let's decrypt their output first.");
                                        Ok(State::GameAction(StateGameAction {
                                            is_my_action: false,
                                            name: s.name,
                                            client: s.client,
                                            ck: s.ck,
                                            user_id: s.user_id,
                                            names: s.names,
                                            view: s.view,
                                        }))
                                    } else {
                                        Err((err, State::ConcludedSetupGame(s)))
                                    }
                                }
                                Err(err) => Err((err, State::ConcludedSetupGame(s))),
                            };
                            return result;
                        }
                        Err((err, State::ConcludedSetupGame(s)))
                    }
                }
            }
            _ => Err((anyhow!("Invalid state for command {}", cmd), state)),
        }
    } else if cmd == &"lay" {
        match state {
            State::ConcludedSetupGame(s) => match cmd_lay(&s.client, s.user_id, &s.view).await {
                Ok(view) => Ok(State::GameAction(StateGameAction {
                    view,
                    is_my_action: true,
                    name: s.name,
                    client: s.client,
                    ck: s.ck,
                    user_id: s.user_id,
                    names: s.names,
                })),
                Err(err) => {
                    if err.to_string().contains("Wrong server state") {
                        let result = match cmd_fhe_ongoing(&s.client).await {
                            Ok(is_ongoing) => {
                                if is_ongoing {
                                    println!("❌ Your action DID NOT take effect!");
                                    println!("❗ Another player took an action first. Let's decrypt their output first.");
                                    Ok(State::GameAction(StateGameAction {
                                        is_my_action: false,
                                        name: s.name,
                                        client: s.client,
                                        ck: s.ck,
                                        user_id: s.user_id,
                                        names: s.names,
                                        view: s.view,
                                    }))
                                } else {
                                    Err((err, State::ConcludedSetupGame(s)))
                                }
                            }
                            Err(err) => Err((err, State::ConcludedSetupGame(s))),
                        };
                        return result;
                    }
                    Err((err, State::ConcludedSetupGame(s)))
                }
            },
            _ => Err((anyhow!("Invalid state for command {}", cmd), state)),
        }
    } else if cmd == &"pickup" {
        match state {
            State::ConcludedSetupGame(s) => match cmd_pickup(&s.client, s.user_id, &s.view).await {
                Ok(view) => Ok(State::GameAction(StateGameAction {
                    view,
                    is_my_action: true,
                    name: s.name,
                    client: s.client,
                    ck: s.ck,
                    user_id: s.user_id,
                    names: s.names,
                })),
                Err(err) => {
                    if err.to_string().contains("Wrong server state") {
                        let result = match cmd_fhe_ongoing(&s.client).await {
                            Ok(is_ongoing) => {
                                if is_ongoing {
                                    println!("❌ Your action DID NOT take effect!");
                                    println!("❗ Another player took an action first. Let's decrypt their output first.");
                                    Ok(State::GameAction(StateGameAction {
                                        is_my_action: false,
                                        name: s.name,
                                        client: s.client,
                                        ck: s.ck,
                                        user_id: s.user_id,
                                        names: s.names,
                                        view: s.view,
                                    }))
                                } else {
                                    Err((err, State::ConcludedSetupGame(s)))
                                }
                            }
                            Err(err) => Err((err, State::ConcludedSetupGame(s))),
                        };
                        return result;
                    }
                    Err((err, State::ConcludedSetupGame(s)))
                }
            },
            _ => Err((anyhow!("Invalid state for command {}", cmd), state)),
        }
    } else if cmd == &"status" {
        match &state {
            State::Init(StateInit { client, .. })
            | State::Setup(StateSetup { client, .. })
            | State::ConcludedRegistration(Registration { client, .. })
            | State::SubmittedSks(Registration { client, .. })
            | State::ConcludedSubmitSks(Registration { client, .. })
            | State::InitGame(StateGame { client, .. })
            | State::SetupGame(StateGame { client, .. })
            | State::ConcludedSetupGame(StateGame { client, .. })
            | State::GameAction(StateGameAction { client, .. })
            | State::CompletedFhe(StateGameAction { client, .. })
            | State::DownloadedOutput(StateDownloadedOutput { client, .. })
            | State::ConcludedDecryptionSubmission(StateDownloadedOutput { client, .. })
            | State::Decrypted(StateDecrypted { client, .. }) => {
                match client.get_dashboard().await {
                    Ok(dashbaord) => {
                        dashbaord.print_presentation();
                        Ok(state)
                    }
                    Err(err) => Err((err, state)),
                }
            }
        }
    } else if cmd.starts_with('#') {
        Ok(state)
    } else {
        Err((anyhow!("Unknown command {}", cmd), state))
    }
}
