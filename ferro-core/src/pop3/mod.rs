mod client;
mod error;
mod stream;

pub use client::Pop3Client;
pub use error::{Pop3Error, Result};

#[cfg(test)]
mod tests;
