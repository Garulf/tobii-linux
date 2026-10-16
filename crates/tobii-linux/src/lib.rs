//! The `tobii-linux` command: the native Eye Tracker 5 driver for Linux.
//!
//! Subcommands, options, outputs, exit codes and log lines are those of the Python command
//! (`python3 -m tobii_linux.cli`) it replaces: `probe` reads USB descriptors, `headpose`
//! prints head-pose samples, `serve` runs the game service (the PRP pipes or the tet-tcp
//! route through the Wine bridge), `demo` serves the browser demo and `setup` starts the
//! GTK setup app. The game service and the demo's HTTP server run on a single-threaded
//! tokio loop; the tracker is read on blocking threads that never wait for their consumers.

pub mod args;
pub mod cli;
pub mod demo;
pub mod error;
pub mod logger;
pub mod paths;
pub mod pyjson;
pub mod service;
pub mod setup;
pub mod tracker;
