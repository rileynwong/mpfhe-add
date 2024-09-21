use crate::{
    compiled::{get_cell, lay_egg, move_player, pickup_egg, add_to_int},
    time,
    types::{GameStateEnc, ServerKeyShare, Word},
    UserAction, UserId,
};
use itertools::Itertools;
use phantom_zone::{aggregate_server_key_shares, set_parameter_set, ParameterSelector};

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

pub(crate) fn evaluate_circuit(
    state: GameStateEnc,
    uas: &[(UserId, UserAction<Word>)],
) -> GameStateEnc {
    let mut state = state.clone();
    for (user_id, ua) in uas {
        println!("Apply action {} for user {}", ua, user_id);
        state = apply_action(state, *user_id, ua);
    }
    state
}

pub(crate) fn apply_action(
    state: GameStateEnc,
    user_id: UserId,
    ua: &UserAction<Word>,
) -> GameStateEnc {
    let mut next_state = state.clone();
    set_parameter_set(PARAMETER);
    match ua {
        UserAction::AddInt { user_int } => {
            // happening on server side
            // TODO: pass arg
            // ?? = add_to_int(&user_int)
            // println!("Adding integer: {}", user_int);
        }
        UserAction::MovePlayer { direction } => {
            next_state.coords[user_id] = Some(move_player(
                &state.coords[user_id].as_ref().expect("exist"),
                &direction,
            ));
        }
        UserAction::LayEgg => {
            next_state.eggs = lay_egg(&state.coords[user_id].as_ref().expect("exist"), &state.eggs);
        }
        UserAction::PickupEgg => {
            next_state.eggs =
                pickup_egg(&state.coords[user_id].as_ref().expect("exist"), &state.eggs);
        }
        UserAction::InitGame { .. }
        | UserAction::SetStartingCoord { .. }
        | UserAction::GetCell { .. }
        | UserAction::ViewInt { .. }
        | UserAction::Done => {
            unreachable!("Shouldn't be in the action queue")
        }
    };
    next_state
}

pub(crate) fn get_user_cell(state: &GameStateEnc, user_id: UserId) -> Word {
    println!("Get cell for user {}", user_id);
    set_parameter_set(PARAMETER);
    let coords = state.coords.iter().flatten().cloned().collect_vec();
    get_cell(
        &coords[user_id],
        &state.eggs,
        &coords.iter().flatten().cloned().collect_vec(),
    )
}
