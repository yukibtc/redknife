use std::{env, io};

use tracing::Level;
use tracing_subscriber::filter::Targets;
use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

pub fn init() -> io::Result<()> {
    let level = match env::var("RK_LOG") {
        Ok(level) => level
            .parse()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid log level"))?,
        Err(_) => Level::INFO,
    };

    let target = Targets::new().with_target("rk", level);

    tracing_subscriber::registry()
        .with(fmt::layer().with_writer(io::stderr).with_target(false))
        .with(target)
        .init();

    Ok(())
}
