#[allow(clippy::module_inception)]
pub mod agent;
pub mod bus;
pub mod commands;
pub mod definition;
pub mod dispatcher;
pub mod generator;
pub mod loop_;
pub mod memory_loader;
pub mod personalization;
pub mod prompt;
pub mod registry;
pub mod runner;

#[allow(unused_imports)]
pub use agent::{Agent, AgentBuilder};
pub use loop_::{process_message, run};
pub use runner::run_persistent_agent;

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_reexport_exists<F>(_value: F) {}

    #[test]
    fn run_function_is_reexported() {
        assert_reexport_exists(run);
        assert_reexport_exists(process_message);
        assert_reexport_exists(loop_::run);
        assert_reexport_exists(loop_::process_message);
    }
}
