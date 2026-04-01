use anyhow::{Result, bail};

pub fn maybe_fail(point: &str) -> Result<()> {
    let Some(raw) = std::env::var("SPROCKET_FAIL_AT").ok() else {
        return Ok(());
    };
    if raw == point {
        bail!("injected failure at {point}");
    }
    Ok(())
}
