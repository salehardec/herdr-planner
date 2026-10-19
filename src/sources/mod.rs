//! Data sources: what the planner reads from the herdr-projects board (its
//! data folder), from herdr and from the commands it runs. Parsers are pure
//! functions over text (tested offline); `runner` runs the commands without
//! blocking the server.

pub mod repos;
pub mod runner;
pub mod workspace;
