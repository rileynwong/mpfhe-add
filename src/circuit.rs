use crate::{time, types::ServerKeyShare};
use phantom_zone::{aggregate_server_key_shares, ParameterSelector};

pub const PARAMETER: ParameterSelector = ParameterSelector::NonInteractiveLTE40PartyExperimental;

/// Server work
/// Warning: global variable change
pub(crate) fn derive_server_key(server_key_shares: &[ServerKeyShare]) {
    let server_key = time!(
        || aggregate_server_key_shares(server_key_shares),
        "Aggregate server key shares"
    );
    server_key.set_server_key();
}
