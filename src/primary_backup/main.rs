use bytes::Bytes;
use clap::Parser;
use lib::network::Receiver;
use lib::network::SimpleSender;
use log::info;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use crate::node::{Message, Node};

mod node;

#[derive(Parser)]
#[clap(author, version, about)]
struct Cli {
    /// The network port of the node where to send txs.
    #[clap(short, long, value_parser, value_name = "UINT", default_value_t = 6100)]
    port: u16,
    /// The network address of the node where to send txs.
    #[clap(short, long, value_parser, value_name = "UINT", default_value_t = IpAddr::V4(Ipv4Addr::LOCALHOST))]
    address: IpAddr,
    /// if running as a replica, this is the address of the primary
    #[clap(long, value_parser, value_name = "ADDR")]
    primary: Option<SocketAddr>,
    /// Store name, useful to have several nodes in same machine.
    #[clap(short, long)]
    db_name: Option<String>,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    info!("Node socket: {}:{}", cli.address, cli.port);

    simple_logger::SimpleLogger::new()
        .env()
        .with_level(log::LevelFilter::Info)
        .init()
        .unwrap();

    let address = SocketAddr::new(cli.address, cli.port);

    let node = if let Some(primary_address) = cli.primary {
        info!(
            "Replica: Running as replica on {}, waiting for commands from the primary node...",
            address
        );

        cli.primary
            .is_some()
            .then(|| info!("Subscribing to primary: {}.", primary_address));

        // TODO: this "Subscribe" message is sent here for testing purposes.
        //       But it shouldn't be here. We should have an initialization loop
        //       inside the actual replica node to handle the response, deal with
        //       errors, and eventually reconnect to a new primary.
        let mut sender = SimpleSender::new();
        let subscribe_message: Bytes = bincode::serialize(&Message::Subscribe { address })
            .unwrap()
            .into();
        sender.send(primary_address, subscribe_message).await;

        let db_name = db_name(cli.db_name.unwrap_or("replica".to_string()));
        Node::backup(&db_name)
    } else {
        info!("Primary: Running as primary on {}.", address);

        let db_name = db_name(cli.db_name.unwrap_or("primary".to_string()));
        Node::primary(&db_name)
    };
    Receiver::spawn(address, node).await.unwrap();
}

fn db_name(name: String) -> String {
    format!(".db_{}", name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lib::command::Command;
    use std::fs;
    use tokio::time::{sleep, Duration};

    // since logger is meant to be initialized once and tests run in parallel,
    // run this before anything because otherwise it errors out
    #[ctor::ctor]
    fn init() {
        simple_logger::SimpleLogger::new()
            .env()
            .with_level(log::LevelFilter::Info)
            .init()
            .unwrap();

        fs::remove_dir_all(db_path("")).unwrap_or_default();
    }

    fn db_path(suffix: &str) -> String {
        format!(".db_test/{}", suffix)
    }

    #[tokio::test]
    async fn test_only_primary_server() {
        let address: SocketAddr = "127.0.0.1:6379".parse().unwrap();
        Receiver::spawn(
            address,
            node::Node::primary(Vec::new(), &db_path("primary1")),
        );
        sleep(Duration::from_millis(10)).await;

        let reply = Command::Get {
            key: "k1".to_string(),
        }
        .send_to(address)
        .await
        .unwrap();
        assert!(reply.is_none());

        let reply = Command::Set {
            key: "k1".to_string(),
            value: "v1".to_string(),
        }
        .send_to(address)
        .await
        .unwrap();
        assert!(reply.is_some());
        assert_eq!("v1".to_string(), reply.unwrap());

        let reply = Command::Get {
            key: "k1".to_string(),
        }
        .send_to(address)
        .await
        .unwrap();
        assert!(reply.is_some());
        assert_eq!("v1".to_string(), reply.unwrap());
    }

    #[tokio::test]
    async fn test_replicated_server() {
        fs::remove_dir_all(".db_test_primary2").unwrap_or_default();
        fs::remove_dir_all(".db_test_backup2").unwrap_or_default();

        let address_primary: SocketAddr = "127.0.0.1:6380".parse().unwrap();
        let address_replica: SocketAddr = "127.0.0.1:6381".parse().unwrap();
        Receiver::spawn(address_replica, node::Node::backup(&db_path("backup2")));
        Receiver::spawn(
            address_primary,
            node::Node::primary(vec![address_replica], &db_path("primary2")),
        );
        sleep(Duration::from_millis(10)).await;

        // get null value
        let reply = Command::Get {
            key: "k1".to_string(),
        }
        .send_to(address_primary)
        .await
        .unwrap();
        assert!(reply.is_none());

        // set a value on primary
        let reply = Command::Set {
            key: "k1".to_string(),
            value: "v1".to_string(),
        }
        .send_to(address_primary)
        .await
        .unwrap();
        assert!(reply.is_some());
        assert_eq!("v1".to_string(), reply.unwrap());

        // get value on primary
        let reply = Command::Get {
            key: "k1".to_string(),
        }
        .send_to(address_primary)
        .await
        .unwrap();
        assert!(reply.is_some());
        assert_eq!("v1".to_string(), reply.unwrap());

        // get value on replica to make sure it was replicated
        let reply = Command::Get {
            key: "k1".to_string(),
        }
        .send_to(address_replica)
        .await
        .unwrap();
        assert!(reply.is_some());
        assert_eq!("v1".to_string(), reply.unwrap());

        // should fail since replica should not respond to set commands
        let reply = Command::Set {
            key: "k3".to_string(),
            value: "_".to_string(),
        }
        .send_to(address_replica)
        .await;
        assert!(reply.is_err());
    }
}