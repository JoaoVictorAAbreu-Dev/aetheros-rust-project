use sha2::{Digest, Sha256};
use std::env;
use std::fs;
use std::io;
use std::io::Read;
use std::io::Write;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const LIMINE_DOWNLOAD_URL: &str =
    "https://github.com/limine-bootloader/limine/releases/download/v12.3.2/limine-binary.zip";
const LIMINE_DOWNLOAD_SHA256: &str =
    "381c55e475a5b6f88afa73bb683fe985fb0d2c4252bfcf02420932c4a112fc8d";
const DIST_DIR: &str = "dist";
const LIMINE_CACHE_DIR: &str = "limine-v12.3.2";
const ESP_DIR: &str = "esp";
const EFI_BOOT_DIR: &str = "EFI/BOOT";
const LIMINE_CONF_NAME: &str = "limine.conf";
const LIMINE_BOOTX64_NAME: &str = "BOOTX64.EFI";
const LIMINE_VARS_NAME: &str = "edk2-x86_64-vars.fd";
const DEFAULT_BOOT_TIMEOUT_SECS: u64 = 20;
const DEFAULT_BOOT_SUCCESS_MARKER: &str = "AetherOS: kernel initialized";
const DEFAULT_MONITOR_PORT: u16 = 45454;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("xtask error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1);
    let command = args.next().unwrap_or_else(|| "help".to_string());

    match command.as_str() {
        "help" => {
            print_help();
            Ok(())
        }
        "build" => build_kernel(),
        "stage" => stage_boot_tree(),
        "run" => run_qemu(args.any(|arg| arg == "--debug")),
        "boot-check" => verify_qemu_boot(),
        "shell-check" => verify_qemu_shell(),
        "test" => run_workspace_tests(),
        other => Err(format!("unknown xtask command: {other}")),
    }
}

fn print_help() {
    println!("AetherOS xtask");
    println!("commands:");
    println!("  build    Build the kernel target");
    println!("  stage    Build the kernel and stage a bootable UEFI tree");
    println!("  run      Stage assets and launch QEMU through Limine");
    println!("  boot-check  Run headless QEMU and verify boot success from serial logs");
    println!("  shell-check Run QEMU, inject shell commands, and capture a framebuffer dump");
    println!("  test     Run formatting, host-safe tests, and kernel checks");
}

fn build_kernel() -> Result<(), String> {
    let mut command = cargo_command();
    command
        .arg("build")
        .arg("--locked")
        .arg("--package")
        .arg("aether-kernel")
        .arg("--target")
        .arg("config/target/x86_64-aetheros.json")
        .arg("-Zbuild-std=core,alloc,compiler_builtins")
        .arg("-Zbuild-std-features=compiler-builtins-mem")
        .arg("-Zjson-target-spec")
        .current_dir(workspace_root());

    run_command(&mut command, "cargo build")
}

fn stage_boot_tree() -> Result<(), String> {
    build_kernel()?;

    let root = workspace_root();
    let limine_dir = ensure_limine_bundle(&root)?;
    let stage_dir = root.join(DIST_DIR).join(ESP_DIR);
    let efi_boot_dir = stage_dir.join(EFI_BOOT_DIR);
    let kernel_source = locate_kernel_binary(&root)?;
    let kernel_dest = stage_dir.join("kernel.elf");
    let limine_cfg_src = locate_limine_config(&root)?;
    let limine_cfg_dest = stage_dir.join(LIMINE_CONF_NAME);
    let limine_cfg_efi_dest = efi_boot_dir.join(LIMINE_CONF_NAME);
    let limine_bootx64_src = limine_dir.join(LIMINE_BOOTX64_NAME);
    let limine_bootx64_dest = efi_boot_dir.join(LIMINE_BOOTX64_NAME);

    recreate_directory(&stage_dir)
        .map_err(|err| format!("failed to create staging tree: {err}"))?;
    fs::create_dir_all(&efi_boot_dir)
        .map_err(|err| format!("failed to create EFI boot directory: {err}"))?;
    fs::copy(&kernel_source, &kernel_dest).map_err(|err| {
        format!(
            "failed to copy kernel binary from {:?}: {err}",
            kernel_source
        )
    })?;
    fs::copy(&limine_cfg_src, &limine_cfg_dest)
        .map_err(|err| format!("failed to copy Limine configuration: {err}"))?;
    fs::copy(&limine_cfg_src, &limine_cfg_efi_dest)
        .map_err(|err| format!("failed to copy EFI-local Limine configuration: {err}"))?;
    fs::copy(&limine_bootx64_src, &limine_bootx64_dest)
        .map_err(|err| format!("failed to copy BOOTX64.EFI: {err}"))?;

    println!("staged kernel: {}", kernel_dest.display());
    println!("staged config: {}", limine_cfg_dest.display());
    println!("staged EFI loader: {}", limine_bootx64_dest.display());
    Ok(())
}

fn run_qemu(debug: bool) -> Result<(), String> {
    stage_boot_tree()?;
    let mut command =
        build_qemu_command(&workspace_root(), &qemu_serial_mode(), &qemu_display_mode())?;

    if debug {
        command.arg("-s").arg("-S");
    }

    run_command(&mut command, "qemu-system-x86_64")
}

fn verify_qemu_boot() -> Result<(), String> {
    stage_boot_tree()?;

    let root = workspace_root();
    let serial_log = root.join(DIST_DIR).join("serial.log");
    let success_marker = boot_success_marker();
    let timeout = Duration::from_secs(boot_timeout_secs());

    if serial_log.exists() {
        fs::remove_file(&serial_log)
            .map_err(|err| format!("failed to clear previous serial log: {err}"))?;
    }

    let serial_mode = format!("file:{}", serial_log.display());
    let mut command = build_qemu_command(&root, &serial_mode, "none")?;
    command.stdout(Stdio::null()).stderr(Stdio::piped());

    let mut child = command
        .spawn()
        .map_err(|err| format!("failed to start qemu-system-x86_64: {err}"))?;

    let started = Instant::now();
    loop {
        if serial_log.contains_marker(&success_marker)? {
            terminate_child(&mut child)?;
            println!(
                "boot-check passed in {:.2}s using marker: {}",
                started.elapsed().as_secs_f32(),
                success_marker
            );
            println!("serial log: {}", serial_log.display());
            return Ok(());
        }

        if let Some(status) = child
            .try_wait()
            .map_err(|err| format!("failed while polling QEMU status: {err}"))?
        {
            let stderr = read_child_stderr(&mut child)?;
            let tail = serial_log.tail_lines(20)?;
            return Err(format!(
                "QEMU exited before boot success. status={status}. marker='{success_marker}'. serial_tail=\n{tail}\nqemu_stderr=\n{stderr}"
            ));
        }

        if started.elapsed() >= timeout {
            let tail = serial_log.tail_lines(20)?;
            terminate_child(&mut child)?;
            return Err(format!(
                "boot-check timed out after {}s waiting for marker '{}'. serial_tail=\n{}",
                timeout.as_secs(),
                success_marker,
                tail
            ));
        }

        thread::sleep(Duration::from_millis(200));
    }
}

fn verify_qemu_shell() -> Result<(), String> {
    stage_boot_tree()?;

    let root = workspace_root();
    let serial_log = root.join(DIST_DIR).join("serial.log");
    let screenshot = root.join(DIST_DIR).join("framebuffer-shell.ppm");
    let success_marker = boot_success_marker();
    let timeout = Duration::from_secs(boot_timeout_secs());
    let monitor_port = qemu_monitor_port();

    remove_if_exists(&serial_log)?;
    remove_if_exists(&screenshot)?;

    let serial_mode = format!("file:{}", serial_log.display());
    let monitor_mode = format!("tcp:127.0.0.1:{monitor_port},server,nowait");
    let mut command = build_qemu_command(&root, &serial_mode, "none")?;
    command
        .arg("-monitor")
        .arg(&monitor_mode)
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    let mut child = command
        .spawn()
        .map_err(|err| format!("failed to start qemu-system-x86_64: {err}"))?;

    let started = Instant::now();
    wait_for_log_marker(&serial_log, &success_marker, timeout, &mut child)?;
    wait_for_log_marker(&serial_log, "AetherOS shell ready", timeout, &mut child)?;

    let mut monitor = connect_monitor(monitor_port, Duration::from_secs(5))?;
    send_shell_command(&mut monitor, "help")?;
    send_shell_command(&mut monitor, "info")?;
    send_shell_command(&mut monitor, "ls")?;
    send_monitor_command(
        &mut monitor,
        &format!("screendump {}", screenshot.display()),
    )?;

    wait_for_log_marker(&serial_log, "Show this command list", timeout, &mut child)?;
    wait_for_log_marker(&serial_log, "AetherOS academic kernel", timeout, &mut child)?;
    wait_for_log_marker(&serial_log, "/README.TXT", timeout, &mut child)?;

    validate_ppm_screenshot(&screenshot)?;
    terminate_child(&mut child)?;

    println!(
        "shell-check passed in {:.2}s. serial log: {} screenshot: {}",
        started.elapsed().as_secs_f32(),
        serial_log.display(),
        screenshot.display()
    );
    Ok(())
}

fn run_workspace_tests() -> Result<(), String> {
    run_command(
        cargo_command()
            .arg("fmt")
            .arg("--all")
            .arg("--")
            .arg("--check")
            .current_dir(workspace_root()),
        "cargo fmt",
    )?;
    run_command(
        cargo_command()
            .arg("check")
            .arg("--locked")
            .arg("--workspace")
            .current_dir(workspace_root()),
        "cargo check",
    )?;
    run_command(
        cargo_command()
            .arg("test")
            .arg("--locked")
            .arg("--package")
            .arg("aether-bootinfo")
            .arg("--package")
            .arg("xtask")
            .current_dir(workspace_root()),
        "cargo test (host-safe packages)",
    )
}

fn wait_for_log_marker(
    serial_log: &PathBuf,
    marker: &str,
    timeout: Duration,
    child: &mut Child,
) -> Result<(), String> {
    let started = Instant::now();
    loop {
        if serial_log.contains_marker(marker)? {
            return Ok(());
        }

        if let Some(status) = child
            .try_wait()
            .map_err(|err| format!("failed while polling QEMU status: {err}"))?
        {
            let stderr = read_child_stderr(child)?;
            let tail = serial_log.tail_lines(20)?;
            return Err(format!(
                "QEMU exited before marker '{marker}'. status={status}. serial_tail=\n{tail}\nqemu_stderr=\n{stderr}"
            ));
        }

        if started.elapsed() >= timeout {
            let tail = serial_log.tail_lines(20)?;
            terminate_child(child)?;
            return Err(format!(
                "timed out after {}s waiting for marker '{}'. serial_tail=\n{}",
                timeout.as_secs(),
                marker,
                tail
            ));
        }

        thread::sleep(Duration::from_millis(200));
    }
}

fn build_qemu_command(
    root: &Path,
    serial_mode: &str,
    display_mode: &str,
) -> Result<Command, String> {
    let qemu =
        find_qemu_binary().ok_or_else(|| "qemu-system-x86_64 executable not found".to_string())?;
    let ovmf_code = find_ovmf_code().ok_or_else(|| {
        "UEFI firmware not found. Install QEMU with edk2/OVMF firmware support.".to_string()
    })?;
    let ovmf_vars = ensure_ovmf_vars(root)?;
    let stage_dir = root.join(DIST_DIR).join(ESP_DIR);

    let mut command = Command::new(qemu);
    command
        .current_dir(root)
        .arg("-machine")
        .arg("q35")
        .arg("-m")
        .arg("256M")
        .arg("-serial")
        .arg(serial_mode)
        .arg("-drive")
        .arg(format!(
            "if=pflash,format=raw,readonly=on,file={}",
            ovmf_code.display()
        ))
        .arg("-drive")
        .arg(format!("if=pflash,format=raw,file={}", ovmf_vars.display()))
        .arg("-drive")
        .arg(format!("format=raw,file=fat:rw:{}", stage_dir.display()))
        .arg("-display")
        .arg(display_mode);

    Ok(command)
}

fn run_command(command: &mut Command, label: &str) -> Result<(), String> {
    let status = command
        .status()
        .map_err(|err| format!("failed to start {label}: {err}"))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("{label} exited with status {status}"))
    }
}

fn terminate_child(child: &mut Child) -> Result<(), String> {
    match child.try_wait() {
        Ok(Some(_)) => Ok(()),
        Ok(None) => {
            child
                .kill()
                .map_err(|err| format!("failed to terminate QEMU after verification: {err}"))?;
            child
                .wait()
                .map_err(|err| format!("failed to wait for QEMU shutdown: {err}"))?;
            Ok(())
        }
        Err(err) => Err(format!("failed to inspect QEMU process state: {err}")),
    }
}

fn connect_monitor(port: u16, timeout: Duration) -> Result<TcpStream, String> {
    let started = Instant::now();
    loop {
        match TcpStream::connect(("127.0.0.1", port)) {
            Ok(stream) => return Ok(stream),
            Err(err) => {
                if started.elapsed() >= timeout {
                    return Err(format!(
                        "failed to connect to QEMU monitor on port {port}: {err}"
                    ));
                }
                thread::sleep(Duration::from_millis(200));
            }
        }
    }
}

fn send_shell_command(stream: &mut TcpStream, command: &str) -> Result<(), String> {
    for key in command.chars() {
        send_monitor_command(stream, &format!("sendkey {}", qemu_sendkey_name(key)?))?;
        thread::sleep(Duration::from_millis(40));
    }
    send_monitor_command(stream, "sendkey ret")?;
    thread::sleep(Duration::from_millis(200));
    Ok(())
}

fn send_monitor_command(stream: &mut TcpStream, command: &str) -> Result<(), String> {
    stream
        .write_all(command.as_bytes())
        .and_then(|_| stream.write_all(b"\n"))
        .map_err(|err| format!("failed to send monitor command '{command}': {err}"))
}

fn qemu_sendkey_name(ch: char) -> Result<String, String> {
    let name = match ch {
        'a'..='z' | '0'..='9' => ch.to_string(),
        'A'..='Z' => format!("shift-{}", ch.to_ascii_lowercase()),
        ' ' => "spc".to_string(),
        '-' => "minus".to_string(),
        '=' => "equal".to_string(),
        '[' => "bracket_left".to_string(),
        ']' => "bracket_right".to_string(),
        ';' => "semicolon".to_string(),
        '\'' => "apostrophe".to_string(),
        ',' => "comma".to_string(),
        '.' => "dot".to_string(),
        '/' => "slash".to_string(),
        '\\' => "backslash".to_string(),
        _ => return Err(format!("unsupported shell-check key: {ch:?}")),
    };

    Ok(name)
}

fn validate_ppm_screenshot(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Err(format!(
            "framebuffer dump was not created: {}",
            path.display()
        ));
    }

    let bytes = fs::read(path)
        .map_err(|err| format!("failed to read framebuffer dump {}: {err}", path.display()))?;
    if bytes.len() < 16 {
        return Err(format!(
            "framebuffer dump is unexpectedly small: {} bytes",
            bytes.len()
        ));
    }

    if !bytes.starts_with(b"P6") {
        return Err(format!(
            "framebuffer dump does not look like a binary PPM image: {}",
            path.display()
        ));
    }

    Ok(())
}

fn read_child_stderr(child: &mut Child) -> Result<String, String> {
    let mut stderr = String::new();
    if let Some(mut handle) = child.stderr.take() {
        handle
            .read_to_string(&mut stderr)
            .map_err(|err| format!("failed to read QEMU stderr: {err}"))?;
    }
    Ok(stderr)
}

fn locate_kernel_binary(root: &Path) -> Result<PathBuf, String> {
    let target_dir = root.join("target").join("x86_64-aetheros").join("debug");
    let candidates = [
        target_dir.join("aether-kernel"),
        target_dir.join("aether-kernel.exe"),
        target_dir.join("aether-kernel.bin"),
    ];

    candidates
        .into_iter()
        .find(|path| path.exists())
        .ok_or_else(|| {
            format!(
                "could not find built kernel binary in {}",
                target_dir.display()
            )
        })
}

fn locate_limine_config(root: &Path) -> Result<PathBuf, String> {
    let candidates = [
        root.join("config").join("boot").join("limine.conf"),
        root.join("config").join("boot").join("limine.cfg"),
    ];

    candidates
        .into_iter()
        .find(|path| path.exists())
        .ok_or_else(|| "could not find Limine configuration in config/boot".to_string())
}

fn ensure_limine_bundle(root: &Path) -> Result<PathBuf, String> {
    let limine_dir = root
        .join(DIST_DIR)
        .join(LIMINE_CACHE_DIR)
        .join("limine-binary");
    let bootx64 = limine_dir.join(LIMINE_BOOTX64_NAME);

    if bootx64.exists() {
        return Ok(limine_dir);
    }

    download_and_extract_limine(root)?;

    if bootx64.exists() {
        Ok(limine_dir)
    } else {
        Err(format!(
            "Limine bundle was downloaded but {} is missing",
            bootx64.display()
        ))
    }
}

fn download_and_extract_limine(root: &Path) -> Result<(), String> {
    let dist_dir = root.join(DIST_DIR);
    let cache_dir = dist_dir.join(LIMINE_CACHE_DIR);
    let archive_path = cache_dir.join("limine-binary.zip");

    fs::create_dir_all(&cache_dir)
        .map_err(|err| format!("failed to create Limine cache directory: {err}"))?;

    download_file(LIMINE_DOWNLOAD_URL, &archive_path)?;
    verify_sha256(&archive_path, LIMINE_DOWNLOAD_SHA256)?;
    extract_zip(&archive_path, &cache_dir)
}

fn verify_sha256(path: &Path, expected: &str) -> Result<(), String> {
    let bytes = fs::read(path)
        .map_err(|err| format!("failed to read {} for verification: {err}", path.display()))?;
    let actual = format!("{:x}", Sha256::digest(bytes));

    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "SHA-256 mismatch for {}: expected {expected}, got {actual}",
            path.display()
        ))
    }
}

fn download_file(url: &str, destination: &Path) -> Result<(), String> {
    if cfg!(windows) {
        let script = format!(
            "$ProgressPreference='SilentlyContinue'; Invoke-WebRequest -Uri '{}' -OutFile '{}'",
            url,
            destination.display()
        );

        return run_command(
            Command::new("powershell")
                .arg("-NoProfile")
                .arg("-Command")
                .arg(script),
            "download Limine bundle",
        );
    }

    if let Some(curl) = find_command("curl") {
        return run_command(
            Command::new(curl)
                .arg("-L")
                .arg(url)
                .arg("-o")
                .arg(destination),
            "download Limine bundle",
        );
    }

    Err("failed to download Limine bundle: neither PowerShell nor curl is available".to_string())
}

fn extract_zip(archive_path: &Path, destination: &Path) -> Result<(), String> {
    let extracted_root = destination.join("limine-binary");
    if extracted_root.exists() {
        fs::remove_dir_all(&extracted_root)
            .map_err(|err| format!("failed to clear Limine cache: {err}"))?;
    }

    if cfg!(windows) {
        let script = format!(
            "Add-Type -AssemblyName System.IO.Compression.FileSystem; \
             [System.IO.Compression.ZipFile]::ExtractToDirectory('{}', '{}')",
            archive_path.display(),
            destination.display()
        );

        return run_command(
            Command::new("powershell")
                .arg("-NoProfile")
                .arg("-Command")
                .arg(script),
            "extract Limine bundle",
        );
    }

    if let Some(unzip) = find_command("unzip") {
        return run_command(
            Command::new(unzip)
                .arg("-o")
                .arg(archive_path)
                .arg("-d")
                .arg(destination),
            "extract Limine bundle",
        );
    }

    Err("failed to extract Limine bundle: unzip support is unavailable".to_string())
}

fn recreate_directory(path: &Path) -> io::Result<()> {
    if path.exists() {
        fs::remove_dir_all(path)?;
    }

    fs::create_dir_all(path)
}

fn find_command(name: &str) -> Option<String> {
    env::var_os("PATH").and_then(|paths| {
        env::split_paths(&paths)
            .flat_map(|path| command_candidates(&path, name))
            .find(|candidate| candidate.exists())
            .map(|candidate| candidate.to_string_lossy().into_owned())
    })
}

fn command_candidates(base: &Path, name: &str) -> Vec<PathBuf> {
    let mut candidates = vec![base.join(name)];
    if cfg!(windows) {
        candidates.push(base.join(format!("{name}.exe")));
        candidates.push(base.join(format!("{name}.cmd")));
        candidates.push(base.join(format!("{name}.bat")));
    }
    candidates
}

fn find_qemu_binary() -> Option<String> {
    find_command("qemu-system-x86_64").or_else(|| {
        let fallback = PathBuf::from(r"C:\Program Files\qemu\qemu-system-x86_64.exe");
        fallback
            .exists()
            .then(|| fallback.to_string_lossy().into_owned())
    })
}

fn find_ovmf_code() -> Option<PathBuf> {
    if let Some(path) = env_path("AETHER_OVMF_CODE") {
        return Some(path);
    }

    let candidates = [
        PathBuf::from(r"C:\Program Files\qemu\share\edk2-x86_64-code.fd"),
        PathBuf::from("/usr/share/OVMF/OVMF_CODE.fd"),
        PathBuf::from("/usr/share/OVMF/OVMF_CODE_4M.fd"),
        PathBuf::from("/usr/share/OVMF/OVMF_CODE.secboot.fd"),
        PathBuf::from("/usr/share/edk2/ovmf/OVMF_CODE.fd"),
        PathBuf::from("/usr/share/edk2/ovmf/OVMF_CODE_4M.fd"),
        PathBuf::from("/usr/share/qemu/OVMF_CODE.fd"),
        PathBuf::from("/usr/share/qemu/OVMF_CODE_4M.fd"),
        PathBuf::from("/usr/share/edk2/x64/OVMF_CODE.fd"),
        PathBuf::from("/opt/homebrew/share/qemu/edk2-x86_64-code.fd"),
    ];

    candidates.into_iter().find(|path| path.exists())
}

fn ensure_ovmf_vars(root: &Path) -> Result<PathBuf, String> {
    let dist_dir = root.join(DIST_DIR);
    let ovmf_vars_dest = dist_dir.join(LIMINE_VARS_NAME);

    if ovmf_vars_dest.exists() {
        return Ok(ovmf_vars_dest);
    }

    fs::create_dir_all(&dist_dir)
        .map_err(|err| format!("failed to create dist directory: {err}"))?;

    let source = find_ovmf_vars_source().ok_or_else(|| {
        "UEFI variable store template not found. Install QEMU with edk2 variable images."
            .to_string()
    })?;

    fs::copy(&source, &ovmf_vars_dest).map_err(|err| {
        format!(
            "failed to copy OVMF vars template from {}: {err}",
            source.display()
        )
    })?;

    Ok(ovmf_vars_dest)
}

fn find_ovmf_vars_source() -> Option<PathBuf> {
    if let Some(path) = env_path("AETHER_OVMF_VARS") {
        return Some(path);
    }

    let candidates = [
        PathBuf::from(r"C:\Program Files\qemu\share\edk2-i386-vars.fd"),
        PathBuf::from("/usr/share/OVMF/OVMF_VARS.fd"),
        PathBuf::from("/usr/share/OVMF/OVMF_VARS_4M.fd"),
        PathBuf::from("/usr/share/edk2/ovmf/OVMF_VARS.fd"),
        PathBuf::from("/usr/share/edk2/ovmf/OVMF_VARS_4M.fd"),
        PathBuf::from("/usr/share/qemu/OVMF_VARS.fd"),
        PathBuf::from("/usr/share/qemu/OVMF_VARS_4M.fd"),
        PathBuf::from("/usr/share/edk2/x64/OVMF_VARS.fd"),
    ];

    candidates.into_iter().find(|path| path.exists())
}

fn qemu_display_mode() -> String {
    env::var("AETHER_QEMU_DISPLAY").unwrap_or_else(|_| "default".to_string())
}

fn qemu_serial_mode() -> String {
    env::var("AETHER_QEMU_SERIAL").unwrap_or_else(|_| "stdio".to_string())
}

fn qemu_monitor_port() -> u16 {
    env::var("AETHER_QEMU_MONITOR_PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_MONITOR_PORT)
}

fn boot_timeout_secs() -> u64 {
    env::var("AETHER_BOOT_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_BOOT_TIMEOUT_SECS)
}

fn boot_success_marker() -> String {
    env::var("AETHER_BOOT_SUCCESS_MARKER")
        .unwrap_or_else(|_| DEFAULT_BOOT_SUCCESS_MARKER.to_string())
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("xtask workspace root")
        .to_path_buf()
}

fn cargo_command() -> Command {
    let mut command = Command::new("cargo");
    if let Some(toolchain) = preferred_rustup_toolchain() {
        command.arg(format!("+{toolchain}"));
    }
    command
}

fn preferred_rustup_toolchain() -> Option<String> {
    if let Ok(toolchain) = env::var("AETHER_RUSTUP_TOOLCHAIN") {
        return if toolchain.trim().is_empty() {
            None
        } else {
            Some(toolchain)
        };
    }

    if cfg!(windows) {
        Some("nightly-2026-06-07-x86_64-pc-windows-gnu".to_string())
    } else {
        Some("nightly-2026-06-07".to_string())
    }
}

fn env_path(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .map(PathBuf::from)
        .filter(|path| path.exists())
}

trait SerialLogExt {
    fn contains_marker(&self, marker: &str) -> Result<bool, String>;
    fn tail_lines(&self, max_lines: usize) -> Result<String, String>;
}

impl SerialLogExt for PathBuf {
    fn contains_marker(&self, marker: &str) -> Result<bool, String> {
        if !self.exists() {
            return Ok(false);
        }

        let content = fs::read_to_string(self)
            .map_err(|err| format!("failed to read serial log {}: {err}", self.display()))?;
        Ok(content.contains(marker))
    }

    fn tail_lines(&self, max_lines: usize) -> Result<String, String> {
        if !self.exists() {
            return Ok("<serial log not created>".to_string());
        }

        let content = fs::read_to_string(self)
            .map_err(|err| format!("failed to read serial log {}: {err}", self.display()))?;
        let lines: Vec<_> = content.lines().collect();
        let start = lines.len().saturating_sub(max_lines);
        Ok(lines[start..].join("\n"))
    }
}

fn remove_if_exists(path: &Path) -> Result<(), String> {
    if path.exists() {
        fs::remove_file(path)
            .map_err(|err| format!("failed to remove {}: {err}", path.display()))?;
    }
    Ok(())
}
