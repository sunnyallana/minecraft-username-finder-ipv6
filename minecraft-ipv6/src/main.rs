use anyhow::{Error, Result};
use chrono::Utc;
use colored::*;
use crossterm::{
    cursor::MoveTo,
    execute,
    style::{Color, Print, ResetColor, SetForegroundColor},
    terminal::{Clear, ClearType},
};
use font8x8::{UnicodeFonts, BASIC_FONTS};
use futures::{stream::FuturesUnordered, StreamExt};
use rand::seq::SliceRandom;
use reqwest::{Client, StatusCode};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::stdout;
use std::net::{IpAddr, Ipv6Addr};
use std::sync::Arc;
use tokio::time::Duration;

struct NameChecker {
    clients: Vec<Arc<Client>>,
    auth_tokens: Vec<String>,
    ip_addresses: Vec<Ipv6Addr>,
}

impl NameChecker {
    fn new(auth_tokens: Vec<String>, ip_addresses: Vec<Ipv6Addr>) -> Self {
        // Create HTTP clients for each IP address
        let clients = ip_addresses
            .iter()
            .map(|addr| {
                let client = Client::builder()
                    .timeout(Duration::from_secs(10))
                    .tcp_keepalive(Duration::from_secs(60))
                    .pool_idle_timeout(Duration::from_secs(60))
                    .pool_max_idle_per_host(50)
                    .local_address(IpAddr::V6(*addr))
                    .build()
                    .expect("Failed to create HTTP client");
                Arc::new(client)
            })
            .collect();

        Self {
            clients,
            auth_tokens,
            ip_addresses,
        }
    }

    async fn check_account_exists(&self, uuid: &str) -> Result<(bool, StatusCode)> {
        // Choose a random client (and thus a random IP address) for the request
        let client = self.clients.choose(&mut rand::thread_rng()).unwrap();
        let resp = client
            .get(&format!(
                "https://sessionserver.mojang.com/session/minecraft/profile/{}",
                uuid
            ))
            .send()
            .await?;

        let status = resp.status();
        Ok((status == StatusCode::NO_CONTENT, status))
    }

    async fn attempt_claim(&self, name: &str) -> Result<(bool, StatusCode)> {
        if let Some(auth_token) = self.auth_tokens.get(0) {
            // Choose a random client (and thus a random IP address) for the request
            let client = self.clients.choose(&mut rand::thread_rng()).unwrap();
            let resp = client
                .put(&format!(
                    "https://api.minecraftservices.com/minecraft/profile/name/{}",
                    name
                ))
                .header("Authorization", format!("Bearer {}", auth_token))
                .send()
                .await?;

            let status = resp.status();
            Ok((status == StatusCode::OK, status))
        } else {
            Err(Error::msg("No authentication tokens available"))
        }
    }

    async fn monitor_uuids(&self, uuid_map: HashMap<String, String>) -> Result<()> {
        let uuid_map = Arc::new(uuid_map);

        loop {
            let mut futures = FuturesUnordered::new();

            for (name, uuid) in uuid_map.iter() {
                let name = name.clone();
                let uuid = uuid.clone();
                let checker = self.clone();

                futures.push(async move {
                    match checker.check_account_exists(&uuid).await {
                        Ok((is_available, check_status)) => {
                            if check_status == StatusCode::TOO_MANY_REQUESTS {
                                (
                                    name,
                                    uuid,
                                    format!("{}", check_status.as_u16()),
                                    false,
                                    Color::Red,
                                )
                            } else if is_available {
                                match checker.attempt_claim(&name).await {
                                    Ok((claimed, claim_status)) => {
                                        let status = if claimed {
                                            format!("CLAIMED ({})", claim_status.as_u16())
                                        } else {
                                            format!("FAILED TO CLAIM ({})", claim_status.as_u16())
                                        };
                                        (name, uuid, status, true, Color::Green)
                                    }
                                    Err(e) => {
                                        (name, uuid, format!("ERROR: {}", e), false, Color::White)
                                    }
                                }
                            } else {
                                (
                                    name,
                                    uuid,
                                    format!("{}", check_status.as_u16()),
                                    false,
                                    Color::White,
                                )
                            }
                        }
                        Err(e) => (name, uuid, format!("ERROR: {}", e), false, Color::White),
                    }
                });
            }

            while let Some((name, uuid, status, _is_important, status_color)) = futures.next().await
            {
                let timestamp = Utc::now().format("%H:%M:%S:%3f").to_string();

                print_colored(&timestamp, Color::Cyan)?;
                print_colored(" | ", Color::DarkGrey)?;
                print_colored(&uuid, Color::DarkGrey)?;
                print_colored(" (", Color::DarkGrey)?;
                print_colored(&name, Color::White)?;
                print_colored(") | ", Color::DarkGrey)?;
                println_colored(&status, status_color)?;
            }
        }
    }
}

impl Clone for NameChecker {
    fn clone(&self) -> Self {
        Self {
            clients: self.clients.clone(),
            auth_tokens: self.auth_tokens.clone(),
            ip_addresses: self.ip_addresses.clone(),
        }
    }
}

async fn get_uuid(username: &str) -> Result<String> {
    let client = Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .expect("Failed to create HTTP client");

    let url = format!(
        "https://api.mojang.com/users/profiles/minecraft/{}",
        username
    );

    let response = client.get(&url).send().await?;

    if response.status().is_success() {
        let json: serde_json::Value = response.json().await?;
        Ok(json["id"]
            .as_str()
            .ok_or_else(|| Error::msg("UUID not found in response"))?
            .to_string())
    } else {
        Err(Error::msg(format!(
            "Failed to get UUID for {}: {}",
            username,
            response.status()
        )))
    }
}

fn print_colored(text: &str, color: Color) -> Result<()> {
    execute!(stdout(), SetForegroundColor(color), Print(text), ResetColor)?;
    Ok(())
}

fn println_colored(text: &str, color: Color) -> Result<()> {
    print_colored(text, color)?;
    println!();
    Ok(())
}

fn hex_to_rgb(hex: &str) -> (u8, u8, u8) {
    let hex = hex.trim_start_matches('#');
    let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0);
    let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
    let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0);
    (r, g, b)
}

fn display_large_text(text: &str) -> Result<()> {
    let mut lines = vec![String::new(); 9];
    for ch in text.chars() {
        if let Some(pattern) = BASIC_FONTS.get(ch) {
            let width = format!("{:08b}", pattern[0].reverse_bits()).len();
            lines[0].push_str(&" ".repeat(width));

            for (i, line) in pattern.iter().enumerate() {
                let line_str = format!("{:08b}", line.reverse_bits())
                    .replace("0", "  ")
                    .replace("1", "[]");
                lines[i + 1].push_str(&line_str);
            }
        }
    }

    let colors = [
        "#FFE6CC", "#FFCCB3", "#FFB399", "#E6B3FF", "#B380FF", "#805AD0", "#4D33A3", "#33266E",
        "#1A204A",
    ];
    for (i, line) in lines.iter().enumerate() {
        let (r, g, b) = hex_to_rgb(colors[i]);
        println!("{}", line.truecolor(r, g, b));
    }
    Ok(())
}

fn get_input(prompt: &str) -> Result<String> {
    print_colored(prompt, Color::DarkGrey)?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    Ok(input.trim().to_string())
}

fn clear_screen() -> Result<()> {
    execute!(stdout(), Clear(ClearType::All), MoveTo(0, 0))?;
    Ok(())
}

fn display_base_ui() -> Result<()> {
    clear_screen()?;
    println!("\n\n");
    display_large_text("EYPClaimer")?;
    println!();
    println_colored("1. Load Auth Tokens from Config", Color::White)?;
    println_colored("2. Get UUIDs", Color::White)?;
    println_colored("3. Run Deletion Claimer", Color::White)?;
    println_colored("4. View Stored UUIDs", Color::White)?;
    println!();
    Ok(())
}

fn display_base_ui_with_prompt(prompt: &str, color: Color) -> Result<()> {
    display_base_ui()?;
    print_colored(prompt, color)?;
    Ok(())
}

fn load_tokens_from_file() -> Result<(Vec<String>, String)> {
    let mut project_dir = env::current_dir()?;
    while !project_dir.join("Cargo.toml").exists() {
        if !project_dir.pop() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "Could not find project root directory",
            )
            .into());
        }
    }
    let tokens_path = project_dir.join("src").join("tokens.txt");
    let content = fs::read_to_string(tokens_path)?;
    let mut seen_tokens = std::collections::HashSet::new();
    let mut unique_tokens = Vec::new();
    let mut duplicate_count = 0;

    for line in content.lines() {
        let token = line.trim().to_string();
        if !token.is_empty() {
            if seen_tokens.insert(token.clone()) {
                unique_tokens.push(token);
            } else {
                duplicate_count += 1;
            }
        }
    }

    let message = if duplicate_count > 0 {
        format!(
            "Loading {} token(s) from tokens.txt (skipped {} duplicate {})",
            unique_tokens.len(),
            duplicate_count,
            if duplicate_count == 1 {
                "token"
            } else {
                "tokens"
            }
        )
    } else {
        format!("Loading {} token(s) from tokens.txt", unique_tokens.len())
    };

    Ok((unique_tokens, message))
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut uuid_map: HashMap<String, String> = HashMap::new();
    let mut auth_tokens = Vec::new();

    display_base_ui_with_prompt("Enter your choice (1-4):", Color::DarkGrey)?;

    loop {
        let choice = get_input("")?;

        match choice.parse::<u32>() {
            Ok(1) => match load_tokens_from_file() {
                Ok((tokens, message)) => {
                    auth_tokens = tokens;
                    display_base_ui()?;
                    println_colored(&message, Color::White)?;
                    println_colored(
                        &format!("Successfully loaded {} Token(s).", auth_tokens.len()),
                        Color::Green,
                    )?;
                    println!();
                    print_colored("Enter your choice ", Color::DarkGrey)?;
                    println_colored("(1-4):", Color::DarkGrey)?;
                }
                Err(e) => {
                    display_base_ui()?;
                    println_colored(&format!("Failed to load tokens: {}", e), Color::Red)?;
                    println!();
                    print_colored("Enter your choice ", Color::DarkGrey)?;
                    println_colored("(1-4):", Color::DarkGrey)?;
                }
            },

            Ok(2) => {
                display_base_ui_with_prompt(
                    "Enter usernames (comma-separated): ",
                    Color::DarkGrey,
                )?;
                let input = get_input("")?;
                let usernames: Vec<String> = input
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();

                display_base_ui()?;
                print_colored("Fetching UUIDs for ", Color::DarkGrey)?;
                print_colored(&usernames.len().to_string(), Color::Green)?;
                println_colored(" usernames...", Color::DarkGrey)?;

                let mut futures = FuturesUnordered::new();
                for username in usernames {
                    futures.push(async move {
                        match get_uuid(&username).await {
                            Ok(uuid) => (username, Ok(uuid)),
                            Err(e) => (username, Err(e)),
                        }
                    });
                }

                while let Some((username, result)) = futures.next().await {
                    match result {
                        Ok(uuid) => {
                            print_colored("Found UUID for ", Color::DarkGrey)?;
                            print_colored(&username, Color::White)?;
                            print_colored(": ", Color::DarkGrey)?;
                            println_colored(&uuid, Color::Green)?;
                            uuid_map.insert(username, uuid);
                        }
                        Err(e) => {
                            print_colored("Failed to get UUID for ", Color::DarkGrey)?;
                            print_colored(&username, Color::White)?;
                            print_colored(": ", Color::DarkGrey)?;
                            println_colored(&e.to_string(), Color::Red)?;
                        }
                    }
                }

                println!();
                print_colored("Enter your choice ", Color::DarkGrey)?;
                println_colored("(1-4):", Color::DarkGrey)?;
            }
            Ok(3) => {
                if auth_tokens.is_empty() {
                    display_base_ui()?;
                    println_colored("Please load auth tokens first (option 1)", Color::Red)?;
                    println!();
                    print_colored("Enter your choice ", Color::DarkGrey)?;
                    println_colored("(1-4):", Color::DarkGrey)?;
                    continue;
                }

                if uuid_map.is_empty() {
                    display_base_ui()?;
                    println_colored(
                        "No UUIDs stored. Please get UUIDs first (option 2)",
                        Color::Red,
                    )?;
                    println!();
                    print_colored("Enter your choice ", Color::DarkGrey)?;
                    println_colored("(1-4):", Color::DarkGrey)?;
                    continue;
                }

                clear_screen()?;
                // Generate a range of IPv6 addresses within the specified subnet
                let subnet_prefix = "2a0e:97c0:3e:ada::";
                let ip_addresses = (0..100).map(|i| {
                    let addr_str = format!("{}{:x}", subnet_prefix, i);
                    addr_str.parse::<Ipv6Addr>().expect("Invalid IPv6 address")
                }).collect::<Vec<_>>();
                let checker = NameChecker::new(auth_tokens.clone(), ip_addresses);
                checker.monitor_uuids(uuid_map.clone()).await?;
            }
            Ok(4) => {
                display_base_ui()?;
                if uuid_map.is_empty() {
                    println_colored("No UUIDs stored.", Color::Red)?;
                } else {
                    println_colored("Stored UUIDs:", Color::DarkGrey)?;
                    for (name, uuid) in &uuid_map {
                        print_colored(name, Color::White)?;
                        print_colored(": ", Color::DarkGrey)?;
                        println_colored(uuid, Color::Green)?;
                    }
                }
                println!();
                print_colored("Enter your choice ", Color::DarkGrey)?;
                println_colored("(1-4):", Color::DarkGrey)?;
            }
            Ok(_) => {
                display_base_ui()?;
                println_colored(
                    "Invalid choice. Please enter a number between 1-4.",
                    Color::Red,
                )?;
                println!();
                print_colored("Enter your choice ", Color::DarkGrey)?;
                println_colored("(1-4):", Color::DarkGrey)?;
            }
            Err(e) => {
                display_base_ui()?;
                print_colored("Invalid input: ", Color::Red)?;
                print_colored(&e.to_string(), Color::Red)?;
                println_colored(". Please enter a number between 1-4.", Color::Red)?;
                println!();
                print_colored("Enter your choice ", Color::DarkGrey)?;
                println_colored("(1-4):", Color::DarkGrey)?;
            }
        }
    }
}