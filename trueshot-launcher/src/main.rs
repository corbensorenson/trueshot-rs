use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;
use std::path::Path;
use colored::*;
use anyhow::{Result, Context, anyhow};
use std::io::Write;

fn main() -> Result<()> {
    println!("{}", "🚀 Initializing TrueShot Launcher...".bold().cyan());

    // 1. Locate Root
    let root = std::env::current_dir()?;
    println!("📂 Workspace: {:?}", root);

    // 2. Build Frontend
    println!("{}", "\n📦 Building Frontend (trueshot-dashboard)...".yellow());
    let dashboard_path = root.join("trueshot-dashboard");
    
    // Check for node
    if which::which("npm").is_err() {
        return Err(anyhow!("NPM not found. Please install Node.js."));
    }

    // Install deps if needed (check node_modules)
    if !dashboard_path.join("node_modules").exists() {
        println!("   Installing dependencies...");
        run_command(&dashboard_path, "npm", &["install"])?;
    }

    // Build
    println!("   Compiling assets...");
    run_command(&dashboard_path, "npm", &["run", "build"])?;

    // 3. Deploy Static
    println!("{}", "\n🚚 Deploying Static Assets...".yellow());
    let static_dest = root.join("static");
    if static_dest.exists() {
        std::fs::remove_dir_all(&static_dest)?;
    }
    
    // Copy dist -> static
    let dist_src = dashboard_path.join("dist");
    let options = fs_extra::dir::CopyOptions::new().content_only(true);
    std::fs::create_dir_all(&static_dest)?;
    fs_extra::dir::copy(dist_src, &static_dest, &options)?;
    println!("   Assets deployed to ./static");

    // 4. Build Backend
    println!("{}", "\n🔧 Compiling Server...".yellow());
    run_command(&root, "cargo", &["build", "-p", "trueshot-server", "--release"])?;

    // 5. Start Server
    println!("{}", "\n🌐 Starting TrueShot Server...".green().bold());
    let server_bin = root.join("target/release/trueshot-server");
    
    let mut child = Command::new(server_bin)
        .current_dir(&root)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .context("Failed to spawn server")?;

    // 6. Wait for Health Check
    println!("   Waiting for health check...");
    let start = std::time::Instant::now();
    let mut ready = false;
    
    while start.elapsed().as_secs() < 10 {
        if let Ok(_) = std::net::TcpStream::connect("127.0.0.1:3000") {
             ready = true;
             break;
        }
        thread::sleep(Duration::from_millis(500));
        print!(".");
        std::io::stdout().flush()?;
    }
    println!();

    if ready {
        println!("{}", "✅ Server Ready!".green());
        println!("{}", "🔗 Opening Browser...".cyan());
        if let Err(e) = webbrowser::open("http://localhost:3000") {
            println!("   Cannot open browser automatically: {}", e);
            println!("   Please visit http://localhost:3000 manually");
        }
    } else {
        println!("{}", "❌ Server failed to respond to health check.".red());
        child.kill()?;
        return Err(anyhow!("Server timed out"));
    }

    println!("{}", "\n🛑 Press Ctrl+C to stop.".dimmed());
    
    // Monitors user interrupt in terminal, or waits for child
    child.wait()?;
    
    Ok(())
}

fn run_command(cwd: &Path, program: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(program)
        .current_dir(cwd)
        .args(args)
        .status()
        .context(format!("Failed to run {} {:?}", program, args))?;
        
    if !status.success() {
        return Err(anyhow!("Command failed: {} {:?}", program, args));
    }
    Ok(())
}
