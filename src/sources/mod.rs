//! Data sources: everything the planner reads from herdr-projects, herdr and
//! the project folders. Parsers are pure functions over text (tested offline);
//! `runner` runs the commands without blocking the server.

pub mod list;
pub mod project_md;
pub mod runner;
pub mod tasks;
pub mod threads;
pub mod workspace;
