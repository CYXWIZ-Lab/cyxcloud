//! Authentication commands
//!
//! Commands for login, logout, and user profile management.

use anyhow::{Context, Result};
use console::style;

use crate::config::{self, Credentials};
use crate::cyxwiz_client::CyxWizClient;
use crate::symbols;

/// Login configuration
pub struct LoginConfig {
    pub email: Option<String>,
}

/// Register configuration
pub struct RegisterConfig {
    pub email: String,
    pub username: Option<String>,
}

/// Run the login command
pub async fn login(cyxwiz_client: &CyxWizClient, config: LoginConfig) -> Result<()> {
    // Check if already logged in
    if let Some(creds) = config::get_valid_credentials() {
        println!(
            "{} Already logged in as {}",
            style("!").yellow(),
            style(&creds.email).cyan()
        );
        println!("Run {} to switch accounts", style("cyxcloud logout").green());
        return Ok(());
    }

    // Get email
    let email = match config.email {
        Some(e) => e,
        None => {
            print!("Email: ");
            std::io::Write::flush(&mut std::io::stdout())?;
            let mut email = String::new();
            std::io::stdin().read_line(&mut email)?;
            email.trim().to_string()
        }
    };

    if email.is_empty() {
        anyhow::bail!("Email is required");
    }

    // Get password
    let password = rpassword::prompt_password("Password: ")
        .context("Failed to read password")?;

    if password.is_empty() {
        anyhow::bail!("Password is required");
    }

    println!("{} Logging in...", style("*").blue());

    // Attempt login
    let auth = cyxwiz_client
        .login(&email, &password)
        .await
        .map_err(|e| anyhow::anyhow!("Login failed: {}", e))?;

    // Calculate expiration time (default 24 hours if not in JWT)
    let expires_at = chrono::Utc::now().timestamp() + 86400;

    // Save credentials
    let credentials = Credentials {
        access_token: auth.token,
        refresh_token: None,
        expires_at,
        user_id: auth.user.id,
        email: auth.user.email,
        username: auth.user.username,
    };

    config::save_credentials(&credentials)
        .context("Failed to save credentials")?;

    println!();
    println!(
        "{} Logged in successfully!",
        style(symbols::CHECK).green().bold()
    );
    println!(
        "Welcome, {}",
        style(credentials.username.as_deref().unwrap_or(&credentials.email)).cyan()
    );

    Ok(())
}

/// Run the logout command
pub async fn logout(cyxwiz_client: &CyxWizClient) -> Result<()> {
    // Load current credentials
    let creds = match config::load_credentials()? {
        Some(c) => c,
        None => {
            println!("{} Not logged in", style("!").yellow());
            return Ok(());
        }
    };

    // Attempt to revoke token on server (ignore errors)
    let _ = cyxwiz_client.logout(&creds.access_token).await;

    // Delete local credentials
    config::delete_credentials()?;

    println!(
        "{} Logged out successfully",
        style(symbols::CHECK).green().bold()
    );

    Ok(())
}

/// Run the whoami command
pub async fn whoami(cyxwiz_client: &CyxWizClient) -> Result<()> {
    // Load credentials
    let creds = match config::load_credentials()? {
        Some(c) => c,
        None => {
            println!("{} Not logged in", style("!").yellow());
            println!("Run {} to login", style("cyxcloud login").green());
            return Ok(());
        }
    };

    // Check if expired
    if creds.is_expired() {
        println!("{} Session expired", style("!").yellow());
        println!("Run {} to login again", style("cyxcloud login").green());
        return Ok(());
    }

    // Try to get profile from server
    let profile = cyxwiz_client
        .get_profile(&creds.access_token)
        .await
        .ok();

    println!();
    println!("{}", style("User Profile").bold().underlined());
    println!("{}", style(symbols::HLINE).dim());

    println!(
        "{:12} {}",
        style("Email:").dim(),
        style(&creds.email).cyan()
    );

    if let Some(username) = &creds.username {
        println!(
            "{:12} {}",
            style("Username:").dim(),
            style(username).cyan()
        );
    }

    println!(
        "{:12} {}",
        style("User ID:").dim(),
        style(&creds.user_id).dim()
    );

    // Show profile info from server if available
    if let Some(p) = profile {
        if let Some(name) = &p.name {
            println!(
                "{:12} {}",
                style("Name:").dim(),
                style(name).cyan()
            );
        }

        if let Some(wallet) = &p.cyx_wallet {
            println!(
                "{:12} {}",
                style("Wallet:").dim(),
                style(&wallet.public_key).cyan()
            );
        } else if let Some(ext_wallet) = &p.external_wallet {
            println!(
                "{:12} {}",
                style("Wallet:").dim(),
                style(ext_wallet).cyan()
            );
        } else {
            println!(
                "{:12} {}",
                style("Wallet:").dim(),
                style("(not linked)").dim()
            );
        }
    }

    println!();
    println!("{}", style("Session").bold());
    println!("{}", style(symbols::HLINE_SHORT).dim());

    let expires = chrono::DateTime::from_timestamp(creds.expires_at, 0)
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
        .unwrap_or_else(|| "Unknown".to_string());

    println!(
        "{:12} {}",
        style("Expires:").dim(),
        style(&expires).dim()
    );

    Ok(())
}

/// Run the register command
pub async fn register(cyxwiz_client: &CyxWizClient, config: RegisterConfig) -> Result<()> {
    // Check if already logged in
    if let Some(creds) = config::get_valid_credentials() {
        println!(
            "{} Already logged in as {}",
            style("!").yellow(),
            style(&creds.email).cyan()
        );
        println!("Run {} first to register a new account", style("cyxcloud logout").green());
        return Ok(());
    }

    // Get password
    let password = rpassword::prompt_password("Password: ")
        .context("Failed to read password")?;

    if password.len() < 8 {
        anyhow::bail!("Password must be at least 8 characters");
    }

    // Confirm password
    let confirm = rpassword::prompt_password("Confirm password: ")
        .context("Failed to read password")?;

    if password != confirm {
        anyhow::bail!("Passwords do not match");
    }

    println!("{} Creating account...", style("*").blue());

    // Attempt registration
    let auth = cyxwiz_client
        .register(&config.email, &password, config.username.as_deref())
        .await
        .map_err(|e| anyhow::anyhow!("Registration failed: {}", e))?;

    // Calculate expiration time (default 24 hours)
    let expires_at = chrono::Utc::now().timestamp() + 86400;

    // Save credentials
    let credentials = Credentials {
        access_token: auth.token,
        refresh_token: None,
        expires_at,
        user_id: auth.user.id,
        email: auth.user.email,
        username: auth.user.username.or(config.username),
    };

    config::save_credentials(&credentials)
        .context("Failed to save credentials")?;

    println!();
    println!(
        "{} Account created successfully!",
        style(symbols::CHECK).green().bold()
    );
    println!(
        "Welcome, {}",
        style(credentials.username.as_deref().unwrap_or(&credentials.email)).cyan()
    );

    Ok(())
}
