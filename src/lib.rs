pub mod api;
pub mod arr;
pub mod budget;
pub mod config;
pub mod executor;
pub mod observe;
pub mod plan;
pub mod qbit;
pub mod rules;
pub mod snapshot;

#[cfg(test)]
mod testkit;

/// An error and its sources on one line; reqwest's own message rarely says
/// what went wrong.
pub fn error_chain(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}
