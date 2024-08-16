use crate::circuit::PARAMETER;
use crate::dashboard::{Dashboard, RegisteredUser};
use itertools::Itertools;
use phantom_zone::{
    evaluator::NonInteractiveMultiPartyCrs,
    keys::CommonReferenceSeededNonInteractiveMultiPartyServerKeyShare, parameters::BoolParameters,
    set_parameter_set, Encryptor, FheBool, KeySwitchWithId, MultiPartyDecryptor,
    NonInteractiveSeededFheBools, SampleExtractor,
};
use rocket::serde::{Deserialize, Serialize};
use rocket::tokio::sync::Mutex;
use rocket::Responder;
use std::collections::HashMap;
use std::fmt::Debug;
use std::fmt::Display;
use std::sync::Arc;
use tabled::Table;
use thiserror::Error;

pub type ClientKey = phantom_zone::ClientKey;
pub type UserId = usize;

pub(crate) type Seed = [u8; 32];
pub(crate) type ServerKeyShare = CommonReferenceSeededNonInteractiveMultiPartyServerKeyShare<
    Vec<Vec<u64>>,
    BoolParameters<u64>,
    NonInteractiveMultiPartyCrs<Seed>,
>;

pub type Word = Vec<FheBool>;
pub(crate) type EncryptedWord = NonInteractiveSeededFheBools<Vec<u64>, Seed>;

/// Decryption share for a word from one user.
pub type DecryptionShare = Vec<u64>;

pub const BOARD_DIM: usize = 4;
pub const BOARD_SIZE: usize = BOARD_DIM * BOARD_DIM;

#[derive(Copy, Clone)]
#[repr(u8)]
pub enum Direction {
    Up = 0,
    Down,
    Left,
    Right,
}

fn u8_to_binary<const N: usize>(v: u8) -> [bool; N] {
    assert!((v as u16) < 2u16.pow(N as u32));
    let mut result = [false; N];
    for i in 0..N {
        if (v >> i) & 1 == 1 {
            result[i] = true;
        }
    }
    result
}

fn coords_to_binary<const N: usize>(x: u8, y: u8) -> [bool; N] {
    let mut result = [false; N];
    for i in 0..N / 2 {
        if (x >> i) & 1 == 1 {
            result[i] = true;
        }
    }
    for i in N / 2..N {
        if (y >> i) & 1 == 1 {
            result[i] = true;
        }
    }
    result
}

#[derive(Debug, Clone)]
pub struct GameStateLocalView {
    user_id: UserId,
    my_coord: (u8, u8),
    eggs_laid: Vec<Vec<bool>>,
}

impl GameStateLocalView {
    // x is the row index, y is the column index
    pub fn new(x: u8, y: u8, user_id: UserId) -> Self {
        Self {
            user_id,
            my_coord: (x, y),
            eggs_laid: vec![vec![false; BOARD_DIM]; BOARD_DIM],
        }
    }

    pub fn move_player(&mut self, dir: Direction) {
        let (x, y) = &mut self.my_coord;
        match dir {
            Direction::Up => *x = (*x - 1 + BOARD_DIM as u8) % BOARD_DIM as u8,
            Direction::Down => *x = (*x + 1) % BOARD_DIM as u8,
            Direction::Left => *y = (*y - 1 + BOARD_DIM as u8) % BOARD_DIM as u8,
            Direction::Right => *y = (*y + 1) % BOARD_DIM as u8,
        }
    }
    pub fn get_egg(&mut self) -> &mut bool {
        let (x, y) = self.my_coord;
        &mut self.eggs_laid[x as usize][y as usize]
    }

    pub fn lay(&mut self) {
        *self.get_egg() = true;
    }

    pub fn pickup(&mut self) {
        *self.get_egg() = false;
    }

    // for row, 0 index starts from the top
    // for column, 0 index starts from the left
    pub fn print(&self) {
        println!("----------------Local View-------------------");
        println!("My coordinates {:?}", self.my_coord);

        let mut data = vec![];
        for _ in 0..BOARD_DIM {
            let cells = (0..BOARD_DIM).map(|_| "_".to_string()).collect_vec();
            data.push(cells)
        }

        let (my_x, my_y) = self.my_coord;
        data[my_x as usize][my_y as usize] = format!("(🐓{})", self.user_id).to_string();

        for x in 0..BOARD_DIM {
            for y in 0..BOARD_DIM {
                if self.eggs_laid[x][y] {
                    data[x][y] = [data[x][y].to_string(), "🥚".to_string()]
                        .join("")
                        .to_string();
                }
            }
        }
        println!("{}", Table::from_iter(data).to_string());
    }

    pub fn print_with_output(&self, output: &[bool]) {
        println!("----------------Global View-------------------");
        println!("(Only my cell is decrypted)");
        println!("My coordinates {:?}", self.my_coord);

        let mut data = vec![];
        for _ in 0..BOARD_DIM {
            let cells = (0..BOARD_DIM).map(|_| "🌫️".to_string()).collect_vec();
            data.push(cells)
        }

        let (my_x, my_y) = self.my_coord;
        let (x, y) = (my_x as usize, my_y as usize);
        data[x][y] = "".to_string();

        for user in 0..4 {
            if output[user] {
                data[x][y] = [data[x][y].to_string(), format!("(🐓{})", user).to_string()].concat()
            }
        }
        if output[4] {
            data[x][y] = [data[x][y].to_string(), "🥚".to_string()].concat()
        }

        println!("{}", Table::from_iter(data).to_string());
    }
}

#[derive(Debug, Clone)]
pub struct GameStateEnc {
    pub coords: Vec<Option<Word>>,
    pub eggs: Word,
}

/// Encrypted input words contributed from one user
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(crate = "rocket::serde")]
pub enum UserAction<T> {
    InitGame { initial_eggs: T },
    SetStartingCoord { starting_coord: T },
    MovePlayer { direction: T },
    LayEgg,
    PickupEgg,
    GetCell,
    Done,
}

impl<T> Display for UserAction<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            UserAction::InitGame { .. } => "InitGame",
            UserAction::SetStartingCoord { .. } => "SetStartingCoord",
            UserAction::MovePlayer { .. } => "MovePlayer",
            UserAction::LayEgg { .. } => "LayEgg",
            UserAction::PickupEgg { .. } => "PickupEgg",
            UserAction::GetCell { .. } => "GetCell",
            UserAction::Done => "Done",
        };
        write!(f, "{}", text)
    }
}

impl UserAction<EncryptedWord> {
    pub fn init_game(ck: &ClientKey, initial_eggs: &[bool]) -> Self {
        let initial_eggs = ck.encrypt(initial_eggs);
        Self::InitGame { initial_eggs }
    }

    pub fn set_starting_coord(ck: &ClientKey, coords: &(u8, u8)) -> Self {
        let (x, y) = coords;
        let starting_coord = ck.encrypt(coords_to_binary::<16>(*x, *y).as_slice());

        Self::SetStartingCoord { starting_coord }
    }

    pub fn move_player(ck: &ClientKey, direction: Direction) -> Self {
        let direction = u8_to_binary::<8>(direction as u8);
        Self::MovePlayer {
            direction: ck.encrypt(direction.as_slice()),
        }
    }

    pub fn unpack(&self, user_id: UserId) -> UserAction<Word> {
        set_parameter_set(PARAMETER);
        match &self {
            UserAction::InitGame { initial_eggs } => UserAction::InitGame {
                initial_eggs: unpack_word(initial_eggs, user_id),
            },
            UserAction::SetStartingCoord { starting_coord } => UserAction::SetStartingCoord {
                starting_coord: unpack_word(starting_coord, user_id),
            },
            UserAction::MovePlayer { direction } => UserAction::MovePlayer {
                direction: unpack_word(direction, user_id),
            },
            UserAction::LayEgg => UserAction::LayEgg,
            UserAction::PickupEgg => UserAction::PickupEgg,
            UserAction::GetCell => UserAction::GetCell,
            UserAction::Done => UserAction::Done,
        }
    }
}

fn unpack_word(word: &EncryptedWord, user_id: UserId) -> Word {
    word.unseed::<Vec<Vec<u64>>>()
        .key_switch(user_id)
        .extract_all()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircuitOutput {
    cell: Word,
}

impl CircuitOutput {
    pub(crate) fn new(cell: Word) -> Self {
        Self { cell }
    }

    pub fn gen_decryption_share(&self, ck: &ClientKey) -> DecryptionShare {
        let dec_share = self
            .cell
            .iter()
            .map(|out_bit| ck.gen_decryption_share(out_bit))
            .collect_vec();
        dec_share
    }

    pub fn decrypt(&self, ck: &ClientKey, dss: &[DecryptionShare]) -> Vec<bool> {
        // A DecryptionShare is user i's contribution to word j.
        // To decrypt word j at bit position k. We need to extract the position k of user i's share.
        let decrypted_bits = self
            .cell
            .iter()
            .enumerate()
            .map(|(bit_k, fhe_bit)| {
                let shares_for_bit_k = dss.iter().map(|user_share| user_share[bit_k]).collect_vec();
                ck.aggregate_decryption_shares(fhe_bit, &shares_for_bit_k)
            })
            .collect_vec();
        decrypted_bits
    }
}

#[derive(Debug, Error)]
pub(crate) enum Error {
    #[error("Wrong server state: expect {expect} but got {got}")]
    WrongServerState { expect: String, got: String },
    #[error("User #{user_id} is unregistered")]
    UnregisteredUser { user_id: usize },
    #[error("The ciphertext from user #{user_id} not found")]
    CipherNotFound { user_id: UserId },
    #[error("Decryption share of {output_id} from user {user_id} not found")]
    DecryptionShareNotFound { output_id: usize, user_id: UserId },
    /// Temporary here
    #[error("Output not ready")]
    OutputNotReady,
    #[error("Init action not performed yet")]
    GameNotInitedYet,
    #[error("Cells not found")]
    CellNotFound,
}

#[derive(Responder)]
pub(crate) enum ErrorResponse {
    #[response(status = 500, content_type = "json")]
    ServerError(String),
    #[response(status = 404, content_type = "json")]
    NotFoundError(String),
}

impl From<Error> for ErrorResponse {
    fn from(error: Error) -> Self {
        match error {
            Error::WrongServerState { .. }
            | Error::CipherNotFound { .. }
            | Error::GameNotInitedYet
            | Error::CellNotFound => ErrorResponse::ServerError(error.to_string()),
            Error::DecryptionShareNotFound { .. }
            | Error::UnregisteredUser { .. }
            | Error::OutputNotReady => ErrorResponse::NotFoundError(error.to_string()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ServerState {
    /// Users are allowed to join the computation
    ReadyForJoining,
    /// We can now accept server key shares
    ReadyForServerKeyShares,
    /// We can now accept starting coordinates
    ReadyForSetupGame,
    ReadyForActions,
    ReadyForRunning,
    RunningFhe,
    CompletedFhe,
}

impl ServerState {
    fn ensure(&self, expect: Self) -> Result<&Self, Error> {
        if *self == expect {
            Ok(self)
        } else {
            Err(Error::WrongServerState {
                expect: expect.to_string(),
                got: self.to_string(),
            })
        }
    }

    fn transit(&mut self, next: Self) {
        *self = next;
    }
}

impl Display for ServerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[[ {:?} ]]", self)
    }
}

pub(crate) type MutexServerStorage = Arc<Mutex<ServerStorage>>;

#[derive(Debug)]
pub(crate) struct ServerStorage {
    pub(crate) seed: Seed,
    pub(crate) state: ServerState,
    pub(crate) users: Vec<UserRecord>,

    pub(crate) game_state: Option<GameStateEnc>,
    pub(crate) action_queue: Vec<(UserId, UserAction<Word>)>,
    // in this case it is the users' cells
    pub(crate) circuit_output: Option<CircuitOutput>,
}

impl ServerStorage {
    pub(crate) fn new(seed: Seed) -> Self {
        Self {
            seed,
            state: ServerState::ReadyForJoining,
            users: vec![],

            game_state: None,
            action_queue: vec![],
            circuit_output: None,
        }
    }

    pub(crate) fn add_user(&mut self, name: &str) -> RegisteredUser {
        let user_id: usize = self.users.len();
        self.users.push(UserRecord {
            id: user_id,
            name: name.to_string(),
            storage: UserStorage::Empty,
        });
        RegisteredUser::new(user_id, name)
    }

    pub(crate) fn ensure(&self, state: ServerState) -> Result<(), Error> {
        self.state.ensure(state)?;
        Ok(())
    }

    pub(crate) fn transit(&mut self, state: ServerState) {
        self.state.transit(state.clone());
        println!("Sever state {}", state);
    }

    pub(crate) fn get_user(&mut self, user_id: UserId) -> Result<&mut UserRecord, Error> {
        self.users
            .get_mut(user_id)
            .ok_or(Error::UnregisteredUser { user_id })
    }

    pub(crate) fn check_cipher_submission(&self) -> bool {
        self.users
            .iter()
            .all(|user| matches!(user.storage, UserStorage::Sks(..)))
    }

    pub(crate) fn check_setup_game_complete(&self) -> bool {
        self.users
            .iter()
            .all(|user| matches!(user.storage, UserStorage::StartingCoords))
    }

    pub(crate) fn get_sks(&mut self) -> Result<Vec<ServerKeyShare>, Error> {
        let mut server_key_shares = vec![];
        for (user_id, user) in self.users.iter_mut().enumerate() {
            if let Some(sks) = user.storage.get_cipher_sks() {
                server_key_shares.push(sks.clone());
            } else {
                return Err(Error::CipherNotFound { user_id });
            }
        }
        Ok(server_key_shares)
    }

    pub(crate) fn get_dashboard(&self) -> Dashboard {
        Dashboard::new(&self.state, &self.users.iter().map_into().collect_vec())
    }
}

#[derive(Debug)]
pub(crate) struct UserRecord {
    pub(crate) id: UserId,
    pub(crate) name: String,
    pub(crate) storage: UserStorage,
}

#[derive(Debug, Clone)]
pub(crate) enum UserStorage {
    Empty,
    Sks(Box<ServerKeyShare>),
    StartingCoords,
    DecryptionShare(Option<DecryptionShare>),
}

impl UserStorage {
    pub(crate) fn get_cipher_sks(&self) -> Option<&ServerKeyShare> {
        match self {
            Self::Sks(sks) => Some(sks),
            _ => None,
        }
    }

    pub(crate) fn get_mut_decryption_share(&mut self) -> Option<&mut Option<DecryptionShare>> {
        match self {
            Self::DecryptionShare(ds) => Some(ds),
            _ => None,
        }
    }
}

/// ([`Word`] index, user_id) -> decryption share
pub type DecryptionSharesMap = HashMap<UserId, DecryptionShare>;

#[derive(Serialize, Deserialize)]
#[serde(crate = "rocket::serde")]
pub(crate) struct SksSubmission {
    pub(crate) user_id: UserId,
    pub(crate) sks: ServerKeyShare,
}

#[derive(Serialize, Deserialize)]
#[serde(crate = "rocket::serde")]
pub(crate) struct DecryptionShareSubmission {
    pub(crate) user_id: UserId,
    pub(crate) decryption_share: DecryptionShare,
}
