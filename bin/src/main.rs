#[macro_use] extern crate nom;
#[macro_use] extern crate clap;
#[macro_use] extern crate serde_derive;
extern crate mio;
extern crate mio_uds;
extern crate serde;
extern crate serde_json;
extern crate time;
extern crate libc;
extern crate slab;
extern crate rand;
extern crate nix;
#[macro_use] extern crate sozu_lib as sozu;
extern crate sozu_command_lib as sozu_command;

mod command;
mod worker;
mod logging;
mod upgrade;

use std::net::{UdpSocket,ToSocketAddrs};
use std::collections::HashMap;
use std::env;
use sozu::network::metrics::{METRICS,ProxyMetrics};
use sozu_command::config::Config;
use clap::{App,Arg,SubCommand};

use command::Worker;
use worker::{begin_worker_process,start_workers};
use upgrade::begin_new_master_process;

fn main() {
  let matches = App::new("sozu")
                        .version(crate_version!())
                        .about("hot reconfigurable proxy")
                        .subcommand(SubCommand::with_name("start")
                                    .about("launch the master process")
                                    .arg(Arg::with_name("config")
                                        .short("c")
                                        .long("config")
                                        .value_name("FILE")
                                        .help("Sets a custom config file")
                                        .takes_value(true)
                                        .required(true)))
                        .subcommand(SubCommand::with_name("worker")
                                    .about("start a worker (internal command, should not be used directly)")
                                    .arg(Arg::with_name("tag").long("tag")
                                         .takes_value(true).required(true).help("worker configuration tag"))
                                    .arg(Arg::with_name("id").long("id")
                                         .takes_value(true).required(true).help("worker identifier"))
                                    .arg(Arg::with_name("fd").long("fd")
                                         .takes_value(true).required(true).help("IPC file descriptor"))
                                    .arg(Arg::with_name("channel-buffer-size").long("channel-buffer-size")
                                         .takes_value(true).required(false).help("Worker's channel buffer size")))
                        .subcommand(SubCommand::with_name("upgrade")
                                    .about("start a new master process (internal command, should not be used directly)")
                                    .arg(Arg::with_name("fd").long("fd")
                                         .takes_value(true).required(true).help("IPC file descriptor"))
                                    .arg(Arg::with_name("channel-buffer-size").long("channel-buffer-size")
                                         .takes_value(true).required(false).help("Worker's channel buffer size")))
                        .get_matches();

  if let Some(matches) = matches.subcommand_matches("worker") {
    let fd  = matches.value_of("fd").expect("needs a file descriptor")
      .parse::<i32>().expect("the file descriptor must be a number");
    let id  = matches.value_of("id").expect("needs a worker id");
    let tag = matches.value_of("tag").expect("needs a configuration tag");
    let buffer_size = matches.value_of("channel-buffer-size")
      .and_then(|size| size.parse::<usize>().ok())
      .unwrap_or(10000);

    begin_worker_process(fd, id, tag, buffer_size);
    return;
  }

  if let Some(matches) = matches.subcommand_matches("upgrade") {
    let fd  = matches.value_of("fd").expect("needs a file descriptor")
      .parse::<i32>().expect("the file descriptor must be a number");
    let buffer_size = matches.value_of("channel-buffer-size")
      .and_then(|size| size.parse::<usize>().ok())
      .unwrap_or(10000);

    begin_new_master_process(fd, buffer_size);
    return;
  }

  let submatches = matches.subcommand_matches("start").expect("unknown subcommand");
  let config_file = submatches.value_of("config").expect("required config file");

  if let Ok(config) = Config::load_from_path(config_file) {
    //FIXME: should have an id for the master too
    logging::setup("MASTER".to_string(), &config.log_level, &config.log_target);
    info!("starting up");

    let metrics_socket = UdpSocket::bind("0.0.0.0:0").unwrap();
    let metrics_host   = (&config.metrics.address[..], config.metrics.port).to_socket_addrs().unwrap().next().unwrap();
    METRICS.lock().unwrap().set_up_remote(metrics_socket, metrics_host);
    let metrics_guard = ProxyMetrics::run();

    let mut proxies:HashMap<String,Vec<Worker>> = HashMap::new();

    for (ref tag, ref ls) in &config.proxies {
      if let Some(workers) = start_workers(&tag, ls) {
        proxies.insert(tag.to_string(), workers);
      }
    };
    info!("created proxies: {:?}", proxies);

    command::start(config, proxies);
    info!("master process stopped");
  } else {
    error!("could not load configuration file at '{}', stopping", config_file);
  }
}

