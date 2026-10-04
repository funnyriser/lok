use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use jito_protos::bundle::Bundle;
use jito_protos::convert::proto_packet_from_versioned_tx;
use jito_protos::searcher::{GetTipAccountsRequest, SendBundleRequest};
use jito_searcher_client::get_searcher_client_auth;
use solana_client::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{read_keypair_file, Signer};
use solana_sdk::system_instruction;
use solana_sdk::transaction::{Transaction, VersionedTransaction};

#[derive(Parser)]
struct Args {
    #[arg(long)]
    keypair_path: String,

    #[arg(long)]
    auth_keypair_path: String,

    #[arg(long, default_value = "https://ny.mainnet.block-engine.jito.wtf")]
    block_engine_url: String,

    #[arg(long, default_value = "https://api.mainnet-beta.solana.com")]
    rpc_url: String,

    #[arg(long, default_value_t = 10_000)]
    tip_lamports: u64,

    #[arg(long, default_value_t = 3)]
    num_bundles: u32,

    // if you don't pass this we just grab one from get_tip_accounts
    #[arg(long)]
    tip_account: Option<String>,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    let payer = read_keypair_file(&args.keypair_path).unwrap();
    let auth_keypair = Arc::new(read_keypair_file(&args.auth_keypair_path).unwrap());

    let rpc_client = RpcClient::new(args.rpc_url.clone());

    let mut searcher_client = get_searcher_client_auth(&args.block_engine_url, &auth_keypair)
        .await
        .expect("failed to connect to block engine");

    println!(
        "connected to block engine, pubkey used for auth: {}",
        auth_keypair.pubkey()
    );

    let tip_account = if let Some(t) = args.tip_account {
        Pubkey::from_str(&t).unwrap()
    } else {
        let accounts = searcher_client
            .get_tip_accounts(GetTipAccountsRequest {})
            .await
            .unwrap()
            .into_inner();
        Pubkey::from_str(&accounts.accounts[0]).unwrap()
    };

    let num_bundles = args.num_bundles;
    let mut landed = 0;

    for i in 0..num_bundles {
        let recent_blockhash = rpc_client.get_latest_blockhash().unwrap();

        let tip_ix = system_instruction::transfer(&payer.pubkey(), &tip_account, args.tip_lamports);

        let tx = Transaction::new_signed_with_payer(
            &[tip_ix],
            Some(&payer.pubkey()),
            &[&payer],
            recent_blockhash,
        );

        let versioned_tx = VersionedTransaction::from(tx);

        let bundle_txs = vec![versioned_tx.clone()];

        let bundle = Bundle {
            header: None,
            packets: bundle_txs
                .iter()
                .map(proto_packet_from_versioned_tx)
                .collect(),
        };

        searcher_client
            .send_bundle(SendBundleRequest {
                bundle: Some(bundle),
            })
            .await
            .unwrap();

        println!("bundle {} sent", i);

        let sig = versioned_tx.signatures[0];
        println!("checking sig {}", sig);

        let mut confirmed = false;
        for _ in 0..20 {
            let statuses = rpc_client.get_signature_statuses(&[sig]).unwrap();

            if let Some(status) = statuses.value[0].clone() {
                if status.err.is_none() {
                    println!("bundle {} landed in slot {}", i, status.slot);
                    confirmed = true;
                    landed += 1;
                    break;
                }
            }

            tokio::time::sleep(Duration::from_millis(500)).await;
        }

        if !confirmed {
            println!("bundle {} vanished into the void, never got confirmed", i);
        }
    }

    let land_rate = (landed as f64 / num_bundles as f64) * 100.0;
    println!(
        "{} of {} bundles landed, {:.1}%",
        landed, num_bundles, land_rate
    );
}
