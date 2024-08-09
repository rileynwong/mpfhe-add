use crate::{
    compiled::{get_cell, lay_egg, move_player, pickup_egg},
    time,
    types::{ServerKeyShare, Word},
    UserAction,
};
use phantom_zone::{aggregate_server_key_shares, ParameterSelector};

pub const PARAMETER: ParameterSelector = ParameterSelector::NonInteractiveLTE4Party;

/// Server work
/// Warning: global variable change
pub(crate) fn derive_server_key(server_key_shares: &[ServerKeyShare]) {
    let server_key = time!(
        || aggregate_server_key_shares(server_key_shares),
        "Aggregate server key shares"
    );
    server_key.set_server_key();
}

pub(crate) fn evaluate_circuit(ua: UserAction<Word>) {
    match ua {
        UserAction::InitGame { initial_eggs } => todo!(),
        UserAction::SetStartingCoords { starting_coords } => todo!(),
        UserAction::MovePlayer { coords, direction } => move_player(&coords, &direction),
        UserAction::LayEgg { coords, eggs } => lay_egg(&coords, &eggs),
        UserAction::PickupEgg { coords, eggs } => pickup_egg(&coords, &eggs),
        UserAction::GetCell {
            coords,
            eggs,
            players,
        } => get_cell(&coords, &eggs, &players),
    };
}
