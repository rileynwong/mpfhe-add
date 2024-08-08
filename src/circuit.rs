use crate::{
    compiled::{karma_add, karma_sub},
    time,
    types::{CircuitInput, CircuitOutput, ServerKeyShare, Word},
};
use itertools::Itertools;
use phantom_zone::{aggregate_server_key_shares, set_parameter_set, ParameterSelector};
use rayon::iter::{IndexedParallelIterator, IntoParallelRefIterator, ParallelIterator};

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

pub(crate) fn evaluate_circuit(cis: &[CircuitInput]) -> CircuitOutput {
    let mut outs = vec![];

    cis.par_iter()
        .enumerate()
        .map(|(my_id, my_ci)| {
            let sent = sum_fhe_dyn(my_ci);
            let received = cis.iter().map(|enc| enc[my_id].clone()).collect_vec();
            let received = sum_fhe_dyn(&received);
            set_parameter_set(PARAMETER);
            karma_sub(&received, &sent)
        })
        .collect_into_vec(&mut outs);
    CircuitOutput::new(outs)
}
