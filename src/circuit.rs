use itertools::Itertools;
use phantom_zone::{
    aggregate_server_key_shares, KeySwitchWithId, ParameterSelector, SampleExtractor,
};

use crate::{time, Cipher, FheBool, ServerKeyShare};

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

/// Server work
pub(crate) fn preprocess_ciphers(ciphers: &[Cipher]) -> Vec<Vec<FheBool>> {
    // Preprocess ciphers
    // 1. Decompression: A cipher is a matrix generated from a seed. The seed is sent through the network as a compression. By calling the `unseed` method we recovered the matrix here.
    // 2. Key Switch: We reencrypt the cipher with the server key for the computation. We need to specify the original signer of the cipher.
    // 3. Extract: A user's encrypted inputs are packed in `BatchedFheUint8` struct. We call `extract_all` method to convert it to `Vec<FheUint8>` for easier manipulation.
    let ciphers = ciphers
        .iter()
        .enumerate()
        .map(|(user_id, cipher)| {
            cipher
                .unseed::<Vec<Vec<u64>>>()
                .key_switch(user_id)
                .extract_all()
        })
        .collect_vec();
    ciphers
}
