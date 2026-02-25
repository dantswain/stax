use tempfile::TempDir;

/// Run a closure with HOME set to a temporary directory.
/// Restores the original HOME afterward.
///
/// IMPORTANT: Only one test should use this at a time because `set_var` is
/// process-global. All tests that mutate HOME are combined into a single
/// `#[test]` function below to avoid races.
fn with_temp_home<F: FnOnce(&std::path::Path)>(f: F) {
    let dir = TempDir::new().expect("failed to create temp dir");
    let original_home = std::env::var("HOME").ok();

    // SAFETY: Only called from a single test at a time (see note above).
    unsafe {
        std::env::set_var("HOME", dir.path());
    }

    f(dir.path());

    unsafe {
        match original_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
    }
}

/// All token_store tests are in one function to avoid parallel HOME mutation.
#[test]
fn test_token_store() {
    // ── store and retrieve ───────────────────────────────────────────────
    with_temp_home(|_| {
        stax::token_store::TokenStore::store_token("ghp_test123").unwrap();

        let token = stax::token_store::TokenStore::get_token();
        assert_eq!(token, Some("ghp_test123".to_string()));
    });

    // ── get_token returns None when nothing stored ───────────────────────
    with_temp_home(|_| {
        let token = stax::token_store::TokenStore::get_token();
        assert_eq!(token, None);
    });

    // ── store overwrites previous token ──────────────────────────────────
    with_temp_home(|_| {
        stax::token_store::TokenStore::store_token("first_token").unwrap();
        stax::token_store::TokenStore::store_token("second_token").unwrap();

        let token = stax::token_store::TokenStore::get_token();
        assert_eq!(token, Some("second_token".to_string()));
    });

    // ── whitespace is trimmed on retrieval ───────────────────────────────
    with_temp_home(|home| {
        let stax_dir = home.join(".stax");
        std::fs::create_dir_all(&stax_dir).unwrap();
        std::fs::write(stax_dir.join("token"), "  ghp_padded  \n").unwrap();

        let token = stax::token_store::TokenStore::get_token();
        assert_eq!(token, Some("ghp_padded".to_string()));
    });

    // ── file permissions (unix only) ─────────────────────────────────────
    #[cfg(unix)]
    with_temp_home(|home| {
        use std::os::unix::fs::PermissionsExt;

        stax::token_store::TokenStore::store_token("ghp_secret").unwrap();

        let token_path = home.join(".stax/token");
        let perms = std::fs::metadata(&token_path).unwrap().permissions();
        assert_eq!(
            perms.mode() & 0o777,
            0o600,
            "token file should be owner-only read/write"
        );

        let dir_perms = std::fs::metadata(home.join(".stax")).unwrap().permissions();
        assert_eq!(
            dir_perms.mode() & 0o777,
            0o700,
            ".stax directory should be owner-only"
        );
    });
}
