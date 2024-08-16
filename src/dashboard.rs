use itertools::Itertools;
use rocket::serde::{Deserialize, Serialize};
use tabled::settings::Style;
use tabled::{Table, Tabled};

use crate::types::{ServerState, UserRecord};
use crate::UserId;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(crate = "rocket::serde")]
pub enum UserStatus {
    IDAcquired,
    SksSubmitted,
    StartingCoordsSubmitted,
    DecryptionShareSubmitted,
}
impl std::fmt::Display for UserStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Tabled)]
#[serde(crate = "rocket::serde")]
pub struct RegisteredUser {
    pub id: UserId,
    pub name: String,
    pub status: UserStatus,
}

impl RegisteredUser {
    pub(crate) fn new(id: UserId, name: &str) -> Self {
        Self {
            id,
            name: name.to_string(),
            status: UserStatus::IDAcquired,
        }
    }
}
impl From<&UserRecord> for RegisteredUser {
    fn from(user: &UserRecord) -> Self {
        use crate::types::UserStorage::*;
        let status = match &user.storage {
            Empty => UserStatus::IDAcquired,
            Sks(_) => UserStatus::SksSubmitted,
            StartingCoords => UserStatus::StartingCoordsSubmitted,
            DecryptionShare(share) => {
                let result = match share {
                    Some(_) => UserStatus::DecryptionShareSubmitted,
                    None => UserStatus::StartingCoordsSubmitted,
                };
                result
            }
        };

        Self {
            id: user.id,
            name: user.name.to_string(),
            status,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Dashboard {
    status: ServerState,
    users: Vec<RegisteredUser>,
    round: usize,
}
impl Dashboard {
    pub(crate) fn new(status: &ServerState, users: &[RegisteredUser], round: usize) -> Self {
        Self {
            status: status.clone(),
            users: users.to_vec(),
            round,
        }
    }

    pub fn get_names(&self) -> Vec<String> {
        self.users
            .iter()
            .map(|reg| reg.name.to_string())
            .collect_vec()
    }

    pub fn get_round(&self) -> usize {
        self.round
    }

    /// APIs for client to check server state
    pub fn is_concluded(&self) -> bool {
        self.status == ServerState::ReadyForServerKeyShares
    }

    pub fn is_submit_sks_complete(&self) -> bool {
        self.status == ServerState::ReadyForSetupGame
    }

    pub fn is_setup_game_complete(&self) -> bool {
        self.status != ServerState::ReadyForSetupGame
    }

    pub fn is_fhe_ongoing(&self) -> bool {
        self.status == ServerState::ReadyForRunning
            || self.status == ServerState::RunningFhe
            || self.status == ServerState::CompletedFhe
    }

    pub fn is_fhe_complete(&self) -> bool {
        self.status == ServerState::CompletedFhe
    }

    pub fn is_decryption_shares_submission_complete(&self, user_id: UserId) -> bool {
        for user in self.users.iter() {
            if user.id != user_id && !matches!(user.status, UserStatus::DecryptionShareSubmitted) {
                return false;
            }
        }
        true
    }

    pub fn is_ready_for_actions(&self, round: usize) -> bool {
        self.status == ServerState::ReadyForActions || self.round > round
    }

    pub fn print_presentation(&self) {
        println!("action no. {}", self.round);
        println!("🤖🧠 {}", self.status);
        let users = Table::new(&self.users)
            .with(Style::ascii_rounded())
            .to_string();
        println!("{}", users);
    }
}
