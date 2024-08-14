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

/// Decryption share with output id
pub type AnnotatedDecryptionShare = (usize, DecryptionShare);

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
    eggs_laid: Vec<bool>,
}

impl GameStateLocalView {
    pub fn new(x: u8, y: u8, user_id: UserId) -> Self {
        Self {
            user_id,
            my_coord: (x, y),
            eggs_laid: vec![false; BOARD_SIZE],
        }
    }
    pub fn move_player(&mut self, dir: Direction) {
        let (x, y) = &mut self.my_coord;
        match dir {
            Direction::Up => *y = y.wrapping_add(1),
            Direction::Down => *y = y.wrapping_sub(1),
            Direction::Left => *x = x.wrapping_sub(1),
            Direction::Right => *x = x.wrapping_add(1),
        }
    }
    pub fn get_egg(&mut self) -> &mut bool {
        let (x, y) = self.my_coord;
        &mut self.eggs_laid[BOARD_DIM * (BOARD_DIM - 1 - (y as usize)) + (x as usize)]
    }

    pub fn lay(&mut self) {
        *self.get_egg() = true;
    }

    pub fn pickup(&mut self) {
        *self.get_egg() = false;
    }

    pub fn print(&self) {
        println!("My coordination {:?}", self.my_coord);
        let mut data = vec![];
        for _ in 0..BOARD_DIM {
            let cells = (0..BOARD_DIM).map(|_| "_".to_string()).collect_vec();
            data.push(cells)
        }

        let (my_x, my_y) = self.my_coord;
        data[BOARD_DIM - 1 - my_y as usize][my_x as usize] =
            format!("(🐓{})", self.user_id).to_string();
        for x in 0..BOARD_DIM {
            for y in 0..BOARD_DIM {
                if self.eggs_laid[BOARD_DIM * (BOARD_DIM - 1 - (y as usize)) + (x as usize)] == true
                {
                    data[BOARD_DIM - 1 - y][x] =
                        [data[BOARD_DIM - 1 - y][x].to_string(), "🥚".to_string()]
                            .join("")
                            .to_string();
                }
            }
        }
        println!("{}", Table::from_iter(data).to_string());
    }

    pub fn print_with_output(&self, output: &[bool]) {
        println!("My coordination {:?}", self.my_coord);
        let mut data = vec![];
        for _ in 0..BOARD_DIM {
            let cells = (0..BOARD_DIM).map(|_| "🌫️".to_string()).collect_vec();
            data.push(cells)
        }
        let (my_x, my_y) = self.my_coord;
        let y = BOARD_DIM - 1 - my_y as usize;
        let x = my_x as usize;
        data[y][x] = "".to_string();
        for user in 0..4 {
            if output[user] == true {
                data[y][x] = [data[y][x].to_string(), format!("(🐓{})", user).to_string()].concat()
            }
        }
        if output[4] == true {
            data[y][x] = [data[y][x].to_string(), "🥚".to_string()].concat()
        }

        println!("{}", Table::from_iter(data).to_string());
    }
}

pub struct GameState {
    /// Player's coordinations. Example: vec![(0u8, 0u8), (2u8, 0u8), (1u8, 1u8), (1u8, 1u8)]
    coords: Vec<PlainCoord>,
    /// example: [false; BOARD_SIZE];
    eggs: Vec<bool>,
}

#[derive(Debug, Clone)]
pub struct GameStateEnc {
    pub coords: Vec<Option<Word>>,
    pub eggs: Word,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(crate = "rocket::serde")]
pub struct PlainCoord {
    pub x: u8,
    pub y: u8,
}

impl PlainCoord {
    pub fn new(x: u8, y: u8) -> PlainCoord {
        PlainCoord { x: x, y: y }
    }

    pub fn from_binnary<const N: usize>(coords: &[bool]) -> PlainCoord {
        let mut x = 0u8;
        let mut y = 0u8;
        for i in (0..N / 2).rev() {
            x = (x << 1) + coords[i] as u8;
        }
        for i in (N / 2..N).rev() {
            y = (y << 1) + coords[i] as u8;
        }
        return PlainCoord { x: x, y: y };
    }

    pub fn to_binary<const N: usize>(&self) -> [bool; N] {
        let mut result = [false; N];
        for i in 0..N / 2 {
            if (self.x >> i) & 1 == 1 {
                result[i] = true;
            }
        }
        for i in N / 2..N {
            if (self.y >> i) & 1 == 1 {
                result[i] = true;
            }
        }
        result
    }
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

pub fn encrypt_plain(ck: &ClientKey, plain: &[bool]) -> EncryptedWord {
    ck.encrypt(plain)
}

fn unpack_word(word: &EncryptedWord, user_id: UserId) -> Word {
    word.unseed::<Vec<Vec<u64>>>()
        .key_switch(user_id)
        .extract_all()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircuitOutput {
    /// Computed karma balance of all users
    cells: Vec<Word>,
}

impl CircuitOutput {
    pub(crate) fn new(cells: Vec<Word>) -> Self {
        Self { cells }
    }

    /// For each output word, a user generates its decryption share
    pub fn gen_decryption_shares(&self, ck: &ClientKey) -> Vec<AnnotatedDecryptionShare> {
        self.cells
            .iter()
            .enumerate()
            .map(|(cell_id, word)| (cell_id, gen_decryption_shares(ck, word)))
            .collect_vec()
    }

    pub fn decrypt(&self, ck: &ClientKey, dss: &[Vec<DecryptionShare>]) -> Vec<Vec<bool>> {
        self.cells
            .iter()
            .zip_eq(dss)
            .map(|(word, shares)| decrypt_word(ck, word, shares))
            .collect_vec()
    }

    /// Get number of outputs
    pub fn n(&self) -> usize {
        self.cells.len()
    }
}

pub fn gen_decryption_shares(ck: &ClientKey, fhe_output: &Word) -> DecryptionShare {
    let dec_shares = fhe_output
        .iter()
        .map(|out_bit| ck.gen_decryption_share(out_bit))
        .collect_vec();
    dec_shares
}

fn decrypt_word(ck: &ClientKey, fhe_output: &Word, shares: &[DecryptionShare]) -> Vec<bool> {
    // A DecryptionShare is user i's contribution to word j.
    // To decrypt word j at bit position k. We need to extract the position k of user i's share.
    let decrypted_bits = fhe_output
        .iter()
        .enumerate()
        .map(|(bit_k, fhe_bit)| {
            let shares_for_bit_k = shares
                .iter()
                .map(|user_share| user_share[bit_k])
                .collect_vec();
            ck.aggregate_decryption_shares(fhe_bit, &shares_for_bit_k)
        })
        .collect_vec();
    decrypted_bits
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
    /// The number of user is determined now.
    /// We can now accept ciphertexts, which depends on the number of users.
    ReadyForInputs,
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
    pub(crate) cells: Option<CircuitOutput>,
}

impl ServerStorage {
    pub(crate) fn new(seed: Seed) -> Self {
        Self {
            seed,
            state: ServerState::ReadyForJoining,
            users: vec![],

            game_state: None,
            action_queue: vec![],
            cells: None,
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

    pub(crate) fn get_sks(&mut self) -> Result<Vec<ServerKeyShare>, Error> {
        let mut server_key_shares = vec![];
        for (user_id, user) in self.users.iter_mut().enumerate() {
            if let Some(sks) = user.storage.get_cipher_sks() {
                server_key_shares.push(sks.clone());
                user.storage = UserStorage::DecryptionShare(None);
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
    DecryptionShare(Option<Vec<AnnotatedDecryptionShare>>),
}

impl UserStorage {
    pub(crate) fn get_cipher_sks(&self) -> Option<&ServerKeyShare> {
        match self {
            Self::Sks(sks) => Some(sks),
            _ => None,
        }
    }

    pub(crate) fn get_mut_decryption_shares(
        &mut self,
    ) -> Option<&mut Option<Vec<AnnotatedDecryptionShare>>> {
        match self {
            Self::DecryptionShare(ds) => Some(ds),
            _ => None,
        }
    }
}

/// ([`Word`] index, user_id) -> decryption share
pub type DecryptionSharesMap = HashMap<(usize, UserId), DecryptionShare>;

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
    /// The user sends decryption share for each [`Word`].
    pub(crate) decryption_shares: Vec<AnnotatedDecryptionShare>,
}

pub fn u64_to_binary<const N: usize>(v: u64) -> [bool; N] {
    assert!((v as u128) < 2u128.pow(N as u32));
    let mut result = [false; N];
    for (i, bit) in result.iter_mut().enumerate() {
        if (v >> i) & 1 == 1 {
            *bit = true;
        }
    }
    result
}

pub fn recover(bits: &[bool]) -> u16 {
    let mut out = 0;
    for (i, bit) in bits.iter().enumerate() {
        out |= (*bit as u16) << i;
    }
    out
}
