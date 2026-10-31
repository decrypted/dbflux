use std::io;

/// Binds the app-control socket, then writes this instance's token. The order
/// matters: a launch that cannot bind because another window holds the socket
/// must leave that window's token file alone, or `dbflux <file>` stops
/// reaching it.
pub(crate) fn claim_app_control<L>(
    bind: impl FnOnce() -> Option<L>,
    write_token: impl FnOnce() -> io::Result<String>,
) -> Result<(L, String), String> {
    let listener = bind().ok_or_else(|| "the IPC socket could not be bound".to_string())?;
    let token =
        write_token().map_err(|error| format!("Failed to initialize IPC auth token: {error}"))?;

    Ok((listener, token))
}

#[cfg(test)]
mod tests {
    use super::claim_app_control;
    use std::cell::Cell;

    #[test]
    fn a_failed_bind_leaves_the_running_windows_token_alone() {
        let token_written = Cell::new(false);

        let result = claim_app_control(
            || None::<()>,
            || {
                token_written.set(true);
                Ok("fresh".to_string())
            },
        );

        assert!(result.is_err());
        assert!(
            !token_written.get(),
            "the token file must not be rewritten when another window holds the socket"
        );
    }

    #[test]
    fn a_successful_bind_writes_the_token() {
        let (listener, token) = claim_app_control(|| Some(7), || Ok("fresh".to_string()))
            .expect("bind and token both succeed");

        assert_eq!((listener, token.as_str()), (7, "fresh"));
    }
}
