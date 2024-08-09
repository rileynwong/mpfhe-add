use crate::circuit::*;
use crate::types::*;
use crate::*;
use anyhow::Error;
use futures::future::join_all;
use itertools::Itertools;
use phantom_zone::MultiPartyDecryptor;
use phantom_zone::{gen_client_key, gen_server_key_share, set_parameter_set};
use rayon::iter::{IntoParallelRefMutIterator, ParallelIterator};
use rocket::{
    serde::{msgpack, Deserialize, Serialize},
    Build, Rocket,
};
use std::{collections::HashMap, time::Duration};
use tokio::time::sleep;

#[derive(Debug, Clone, Serialize, Deserialize)]
// We're not sending the User struct in rockets. This macro is here just for Serde reasons
#[serde(crate = "rocket::serde")]
struct User {
    name: String,
    // step 0: get seed
    seed: Option<Seed>,
    // step 0.5: gen client key
    ck: Option<ClientKey>,
    // step 1: get userID
    id: Option<UserId>,
    total_users: Option<usize>,
    // step 2: assign starting coordinates
    starting_coords: Option<(u8, u8)>,
    // step 3: gen key and cipher
    server_key: Option<ServerKeyShare>,
    // step 4: get FHE output
    fhe_out: Option<CircuitOutput>,
    // step 5: derive decryption shares
    decryption_shares: DecryptionSharesMap,
}

impl User {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            seed: None,
            ck: None,
            id: None,
            total_users: None,
            starting_coords: None,
            server_key: None,
            fhe_out: None,
            decryption_shares: HashMap::new(),
        }
    }

    fn assign_seed(&mut self, seed: Seed) -> &mut Self {
        self.seed = Some(seed);
        self
    }

    fn gen_client_key(&mut self) -> &mut Self {
        self.ck = Some(gen_client_key());
        self
    }

    fn set_id(&mut self, id: usize) -> &mut Self {
        self.id = Some(id);
        self
    }

    fn set_total_users(&mut self, total_users: usize) -> &mut Self {
        self.total_users = Some(total_users);
        self
    }

    fn assign_starting_coords(&mut self, coords: &(u8, u8)) -> &mut Self {
        self.starting_coords = Some(coords.clone());
        self
    }

    fn gen_server_key_share(&mut self) -> &mut Self {
        let server_key = gen_server_key_share(
            self.id.unwrap(),
            self.total_users.unwrap(),
            self.ck.as_ref().unwrap(),
        );
        self.server_key = Some(server_key);
        self
    }

    fn set_fhe_out(&mut self, fhe_out: CircuitOutput) -> &mut Self {
        self.fhe_out = Some(fhe_out);
        self
    }

    /// Populate decryption_shares with my shares
    fn gen_decryption_shares(&mut self) -> &mut Self {
        let ck = self.ck.as_ref().expect("already exists");
        let fhe_out = self.fhe_out.as_ref().expect("exists");
        let my_id = self.id.expect("exists");

        let my_decryption_shares = fhe_out.gen_decryption_shares(ck);
        for (out_id, share) in my_decryption_shares.iter() {
            self.decryption_shares
                .insert((*out_id, my_id), share.to_vec());
        }
        self
    }

    fn get_my_shares(&self) -> Vec<AnnotatedDecryptionShare> {
        let total_users = self.total_users.expect("exist");
        let my_id = self.id.expect("exists");
        (0..total_users)
            .filter_map(|output_id| {
                if output_id == my_id {
                    return None;
                };

                let share = self
                    .decryption_shares
                    .get(&(output_id, my_id))
                    .expect("exists")
                    .to_owned();
                Some((output_id, share))
            })
            .collect_vec()
    }

    fn decrypt_everything(&self) -> Vec<Vec<bool>> {
        let total_users = self.total_users.expect("exist");
        let ck = self.ck.as_ref().expect("already exists");
        let co = self.fhe_out.as_ref().expect("exists");

        let dss = (0..co.n())
            .map(|output_id| {
                (0..total_users)
                    .map(|user_id| {
                        self.decryption_shares
                            .get(&(output_id, user_id))
                            .expect("exists")
                            .to_owned()
                    })
                    .collect_vec()
            })
            .collect_vec();
        co.decrypt(ck, &dss)
    }
}

impl WebClient {
    pub(crate) async fn new_test(rocket: Rocket<Build>) -> Result<Self, Error> {
        let client = rocket::local::asynchronous::Client::tracked(rocket).await?;
        Ok(Self::Test(Box::new(client)))
    }
}

async fn run_flow_with_n_users(total_users: usize) -> Result<(), Error> {
    let client = WebClient::new_test(rocket()).await.unwrap();

    let mut users = (0..total_users)
        .map(|i| User::new(&format!("User {i}")))
        .collect_vec();

    println!("acquire seeds");

    // Acquire seeds
    for user in users.iter_mut() {
        let seed = client.get_seed().await.unwrap();
        user.assign_seed(seed);
        user.gen_client_key();
    }

    println!("register users");

    // Register
    for user in users.iter_mut() {
        let reg = client.register(&user.name).await.unwrap();
        user.set_id(reg.id);
    }

    for user in users.iter_mut() {
        let dashboard = client.get_dashboard().await.unwrap();
        user.set_total_users(dashboard.get_names().len());
    }

    println!("generate and submit server key share");

    // Generate and submit server key share
    for user in users.iter_mut() {
        set_parameter_set(PARAMETER);
        println!("{} Gen cipher", user.name);

        time!(
            || {
                user.gen_server_key_share();
            },
            format!("{} Gen server key share", user.name)
        );

        let user_id = user.id.unwrap();
        let sks = user.server_key.as_ref().unwrap();
        if user_id == 0 {
            let sks = msgpack::to_vec(sks).unwrap();
            println!("sks size {}", sks.len());
        }

        println!("{} Submit server key", user.name);
        client.submit_sks(user_id, &sks).await.unwrap();
        // Drop here to save mem
        user.server_key = None;
    }

    println!("user 0 calls init game");

    // User 0 encrypt initial eggs
    let initial_eggs = [false; BOARD_SIZE];
    let ck = users[0].ck.as_ref().unwrap();
    client.init_game(ck, 0, &initial_eggs);

    println!("users call set starting coords");

    // Assign starting coords
    let users_coords = vec![(0u8, 0u8), (0u8, 0u8), (0u8, 0u8), (0u8, 0u8)];
    for user in users.iter_mut() {
        user.assign_starting_coords(&users_coords[user.id.unwrap()]);
    }

    for (i, user) in users.iter_mut().enumerate() {
        let ck = user.ck.as_ref().unwrap();
        client.set_starting_coords(ck, i, &[user.starting_coords.unwrap()]);
    }

    println!("round 1 start");
    println!("each user submit an action");

    let directions = vec![Direction::Up, Direction::Down];
    for (i, user) in users.iter_mut().enumerate() {
        let ck = user.ck.as_ref().unwrap();
        if i == 0 || i == 1 {
            client.move_player(ck, i, directions[i]);
        } else if i == 2 {
            client.lay_egg(i);
        } else {
            client.pickup_egg(i);
        }
    }

    println!("any user calls trigger the run");

    // Admin runs the FHE computation
    client.trigger_fhe_run().await.unwrap();
    while client.trigger_fhe_run().await.unwrap() != ServerState::CompletedFhe {
        sleep(Duration::from_secs(1)).await
    }

    println!("users get fhe output and decrypt shares");

    // Users get FHE output, generate decryption shares, and submit decryption shares
    for user in users.iter_mut() {
        let fhe_output = client.get_fhe_output().await.unwrap();

        user.set_fhe_out(fhe_output);
        user.gen_decryption_shares();

        client
            .submit_decryption_shares(user.id.expect("exist now"), &&user.get_my_shares())
            .await
            .unwrap();
    }

    // TODO: should not hard code this
    let correct_ouput = [
        [true, false, false, false, false],
        [false, true, false, false, false],
        [false, false, true, true, false],
        [false, false, true, true, false],
    ];

    // Each user decrypt thier own cell
    println!("Users decrypt their own cell");
    for user in users {
        let decrypted_outs = user.decrypt_everything();
        println!("{} sees {:?}", user.name, decrypted_outs);
        assert_eq!(decrypted_outs, correct_ouput);
    }
    Ok(())
}

#[rocket::async_test]
async fn full_flow() {
    // Need to fix the global variable thing to allow multiple flow run
    // run_flow_with_n_users(2).await.unwrap();
    // run_flow_with_n_users(3).await.unwrap();
    run_flow_with_n_users(4).await.unwrap();
}
