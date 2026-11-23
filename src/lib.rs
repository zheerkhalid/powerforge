//! PowerForge: battery health, power plans, keep-awake and sleep/wake
//! diagnostics for Windows, in plain language.
//!
//! The logic lives in this library so that it can be unit-tested; `main.rs`
//! only parses the command line.
pub mod alerts;
pub mod autoswitch;
pub mod awake;
pub mod battery;
pub mod batteryreport;
pub mod diag;
pub mod elevate;
pub mod energy;
pub mod engine;
pub mod events;
pub mod export;
pub mod health;
pub mod history;
pub mod impact;
pub mod plans;
pub mod powercfg;
pub mod selftest;
pub mod settings;
pub mod sys;
pub mod tray;
pub mod ui;
pub mod util;
