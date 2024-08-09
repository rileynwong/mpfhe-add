use crate::dashboard::{Dashboard, RegisteredUser};
use itertools::Itertools;
use phantom_zone::{
    evaluator::NonInteractiveMultiPartyCrs,
    keys::CommonReferenceSeededNonInteractiveMultiPartyServerKeyShare, parameters::BoolParameters,
    Encryptor, FheBool, KeySwitchWithId, MultiPartyDecryptor, NonInteractiveSeededFheBools,
    SampleExtractor,
};
use rocket::serde::{Deserialize, Serialize};
use rocket::tokio::sync::Mutex;
use rocket::Responder;
use std::collections::HashMap;
use std::fmt::Debug;
use std::fmt::Display;
use std::sync::Arc;
use thiserror::Error;

pub type Score = i16;

pub type ClientKey = phantom_zone::ClientKey;
pub type UserId = usize;

pub(crate) type Seed = [u8; 32];
pub(crate) type ServerKeyShare = CommonReferenceSeededNonInteractiveMultiPartyServerKeyShare<
    Vec<Vec<u64>>,
    BoolParameters<u64>,
    NonInteractiveMultiPartyCrs<Seed>,
>;
pub(crate) type Word = Vec<FheBool>;
pub(crate) type CircuitInput = Vec<Word>;
/// Decryption share for a word from one user.
pub(crate) type DecryptionShare = Vec<u64>;

pub type Coord = u8;

type PlainWord = i16;
pub(crate) type EncryptedWord = NonInteractiveSeededFheBools<Vec<u64>, Seed>;

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

pub struct GameState {
    /// Player's coordinations. Example: vec![(0u8, 0u8), (2u8, 0u8), (1u8, 1u8), (1u8, 1u8)]
    coords: Vec<(u8, u8)>,
    /// example: [false; BOARD_SIZE];
    eggs: Vec<bool>,
}

#[derive(Debug, Clone)]
pub struct GameStateEnc {
    pub coords: Vec<Word>,
    pub eggs: Word,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(crate = "rocket::serde")]
pub struct PlainCoord {
    pub x: u8,
    pub y: u8,
}

impl PlainCoord {
    fn new(x: u8, y: u8) -> PlainCoord {
        PlainCoord { x: x, y: y }
    }

    fn from_binnary<const N: usize>(coords: &[bool]) -> PlainCoord {
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

    fn to_binary<const N: usize>(&self) -> [bool; N] {
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
    SetStartingCoords { starting_coords: Vec<T> },
    MovePlayer { coords: T, direction: T },
    LayEgg { coords: T, eggs: T },
    PickupEgg { coords: T, eggs: T },
    GetCell { coords: T, eggs: T, players: T },
}

impl<T> Display for UserAction<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            UserAction::InitGame { .. } => "InitGame",
            UserAction::SetStartingCoords { .. } => "SetStartingCoords",
            UserAction::MovePlayer { .. } => "MovePlayer",
            UserAction::LayEgg { .. } => "LayEgg",
            UserAction::PickupEgg { .. } => "PickupEgg",
            UserAction::GetCell { .. } => "GetCell",
        };
        write!(f, "{}", text)
    }
}

impl UserAction<EncryptedWord> {
    pub fn from_plain(ck: &ClientKey, karma: &[PlainWord]) -> Self {
        todo!();
        let cipher = karma
            .iter()
            .map(|score| encrypt_plain(ck, *score))
            .collect_vec();
    }

    pub fn set_starting_coords(ck: &ClientKey, coords: &[(u8, u8)]) -> Self {
        let starting_coords = coords
            .iter()
            .map(|(x, y)| ck.encrypt(coords_to_binary::<16>(*x, *y).as_slice()))
            .collect_vec();

        Self::SetStartingCoords { starting_coords }
    }

    pub fn unpack(&self, user_id: UserId) -> UserAction<Word> {
        match self {
            UserAction::InitGame { initial_eggs } => UserAction::InitGame {
                initial_eggs: unpack_word(initial_eggs, user_id),
            },
            UserAction::SetStartingCoords { starting_coords } => UserAction::SetStartingCoords {
                starting_coords: starting_coords
                    .iter()
                    .map(|word| unpack_word(word, user_id))
                    .collect_vec(),
            },
            UserAction::MovePlayer { coords, direction } => UserAction::MovePlayer {
                coords: unpack_word(coords, user_id),
                direction: unpack_word(direction, user_id),
            },
            UserAction::LayEgg { coords, eggs } => UserAction::LayEgg {
                coords: unpack_word(coords, user_id),
                eggs: unpack_word(eggs, user_id),
            },
            UserAction::PickupEgg { coords, eggs } => UserAction::PickupEgg {
                coords: unpack_word(coords, user_id),
                eggs: unpack_word(eggs, user_id),
            },
            UserAction::GetCell {
                coords,
                eggs,
                players,
            } => UserAction::GetCell {
                coords: unpack_word(coords, user_id),
                eggs: unpack_word(eggs, user_id),
                players: unpack_word(players, user_id),
            },
        }
    }
}

impl UserAction<Word> {}

fn encrypt_plain(ck: &ClientKey, plain: PlainWord) -> EncryptedWord {
    let plain = u64_to_binary::<32>(plain as u64);
    ck.encrypt(plain.as_slice())
}

fn unpack_word(word: &EncryptedWord, user_id: UserId) -> Word {
    word.unseed::<Vec<Vec<u64>>>()
        .key_switch(user_id)
        .extract_all()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircuitOutput {
    /// Computed karma balance of all users
    karma_balance: Vec<Word>,
}

impl CircuitOutput {
    pub(crate) fn new(karma_balance: Vec<Word>) -> Self {
        Self { karma_balance }
    }

    /// For each output word, a user generates its decryption share
    pub fn gen_decryption_shares(&self, ck: &ClientKey) -> Vec<DecryptionShare> {
        self.karma_balance
            .iter()
            .map(|word| gen_decryption_shares(ck, word))
            .collect_vec()
    }

    pub fn decrypt(&self, ck: &ClientKey, dss: &[Vec<DecryptionShare>]) -> Vec<PlainWord> {
        self.karma_balance
            .iter()
            .zip_eq(dss)
            .map(|(word, shares)| decrypt_word(ck, word, shares))
            .collect_vec()
    }

    /// Get number of outputs
    pub fn n(&self) -> usize {
        self.karma_balance.len()
    }
}

fn gen_decryption_shares(ck: &ClientKey, fhe_output: &Word) -> DecryptionShare {
    let dec_shares = fhe_output
        .iter()
        .map(|out_bit| ck.gen_decryption_share(out_bit))
        .collect_vec();
    dec_shares
}

fn decrypt_word(ck: &ClientKey, fhe_output: &Word, shares: &[DecryptionShare]) -> PlainWord {
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
    recover(&decrypted_bits) as i16
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
            | Error::GameNotInitedYet => ErrorResponse::ServerError(error.to_string()),
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
    pub(crate) cells: Option<Vec<Word>>,
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
        self.state.transit(state)
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
    DecryptionShare(Option<Vec<DecryptionShare>>),
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
    ) -> Option<&mut Option<Vec<DecryptionShare>>> {
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
    pub(crate) decryption_shares: Vec<DecryptionShare>,
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
