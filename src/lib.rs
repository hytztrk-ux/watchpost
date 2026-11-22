//! watchpost – a single-binary uptime and service monitor.
//!
//! The crate is split into:
//!
//! * [`config`]    – loading and validating `watchpost.toml`,
//! * [`monitor`]   – HTTP, TCP and TLS probes,
//! * [`alert`]     – the state machine that decides when to notify,
//! * [`scheduler`] – runs probes on their intervals,
//! * [`store`]     – SQLite history, uptime and incidents,
//! * [`notify`]    – Telegram, Discord, webhook and e-mail delivery,
//! * [`web`]       – the status page, JSON API and health endpoint.

pub mod alert;
pub mod config;
pub mod i18n;
pub mod monitor;
pub mod notify;
pub mod scheduler;
pub mod store;
pub mod web;
