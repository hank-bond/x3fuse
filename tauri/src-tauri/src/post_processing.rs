use crate::{
    metadata::ExifTool,
    model::{BatchSettings, ColorProfile, OutputFormat},
    storage::Logs,
};
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    io::{Seek, Write},
    path::Path,
    process::Stdio,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
    task::JoinSet,
};

const LOG_LIMIT: usize = 256 * 1024;

pub fn enabled(settings: &BatchSettings) -> bool {
    settings.output_format == OutputFormat::Dng
        && !settings.dng_post_processing_command.trim().is_empty()
}

fn request(
    input: &Path,
    output: &Path,
    settings: &BatchSettings,
    resources: &Path,
    metadata: Value,
) -> Value {
    let color = match settings.color_profile {
        ColorProfile::Srgb => "sRGB",
        ColorProfile::AdobeRgb => "AdobeRGB",
        ColorProfile::ProPhotoRgb => "ProPhotoRGB",
        ColorProfile::None => "None",
    };
    json!({
        "schema_version": 1,
        "input_path": input,
        "output_path": output,
        "output_format": "dng",
        "metadata": metadata,
        "conversion_settings": {
            "compress": settings.compress,
            "denoise_intensity": settings.denoise_intensity,
            "highlight_recovery": settings.dng_highlight_recovery,
            "look_profile_path": settings.dng_look.path(resources),
            "color_profile": color,
        }
    })
}

/// Called only after publication. Errors never remove or roll back the DNG.
pub async fn apply(
    input: &Path,
    output: &Path,
    settings: &BatchSettings,
    resources: &Path,
    exif: &ExifTool,
    cancel: &AtomicBool,
) -> Result<(), String> {
    if !enabled(settings) {
        return Ok(());
    }
    let metadata = match exif.hook_metadata(input, cancel).await {
        Ok(metadata) => metadata,
        Err(error) => {
            exif.logs
                .write("debug", format!("Post-processing metadata: {error}"));
            json!({})
        }
    };
    let payload = request(input, output, settings, resources, metadata);
    run(
        &settings.dng_post_processing_command,
        &payload,
        output.parent().ok_or("Missing output directory")?,
        &exif.logs,
        cancel,
    )
    .await?;
    let info =
        std::fs::metadata(output).map_err(|e| format!("Output missing after command: {e}"))?;
    if !info.is_file() || info.len() == 0 {
        return Err("Command did not leave a nonempty output file".into());
    }
    Ok(())
}

struct Capture {
    bytes: Vec<u8>,
    truncated: bool,
}

async fn capture(mut reader: impl AsyncRead + Unpin) -> std::io::Result<Capture> {
    let mut tail = VecDeque::with_capacity(LOG_LIMIT);
    let mut buffer = [0; 8192];
    let mut truncated = false;
    loop {
        let count = reader.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        let excess = (tail.len() + count).saturating_sub(LOG_LIMIT);
        if excess != 0 {
            tail.drain(..excess);
            truncated = true;
        }
        tail.extend(&buffer[..count]);
    }
    Ok(Capture {
        bytes: tail.into(),
        truncated,
    })
}

fn shell(script: &str) -> Result<Command, String> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let mut command = Command::new("/bin/sh");
        command.args(["-c", script]);
        command.as_std_mut().process_group(0);
        Ok(command)
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let mut command = Command::new(system_tool("cmd.exe")?);
        command.args(["/D", "/S", "/C"]);
        command.as_std_mut().raw_arg(format!("\"{script}\""));
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
        Ok(command)
    }
}

#[cfg(windows)]
fn system_tool(name: &str) -> Result<std::path::PathBuf, String> {
    let root = std::env::var_os("SystemRoot").ok_or("Windows system directory is unavailable")?;
    Ok(std::path::PathBuf::from(root).join("System32").join(name))
}

#[cfg(unix)]
fn signal_group(pid: u32, signal: i32) -> std::io::Result<()> {
    unsafe extern "C" {
        fn kill(pid: std::ffi::c_int, signal: std::ffi::c_int) -> std::ffi::c_int;
    }
    let pid = i32::try_from(pid).map_err(std::io::Error::other)?;
    // The child starts a new process group with its own PID before executing the shell.
    if unsafe { kill(-pid, signal) } == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(3) {
        Ok(())
    } else {
        Err(error)
    } // ESRCH
}

#[cfg(unix)]
struct ProcessGroup(Option<u32>);

#[cfg(unix)]
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        if let Some(pid) = self.0 {
            let _ = signal_group(pid, 9);
        }
    }
}

async fn terminate(pid: u32) -> Result<(), String> {
    #[cfg(unix)]
    {
        let term = signal_group(pid, 15);
        // Do not reap the shell until after escalation: its PID still identifies our group.
        tokio::time::sleep(Duration::from_secs(1)).await;
        signal_group(pid, 9).map_err(|e| e.to_string())?;
        term.map_err(|e| e.to_string())
    }
    #[cfg(windows)]
    {
        let status = Command::new(system_tool("taskkill.exe")?)
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .creation_flags(0x08000000)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .map_err(|e| e.to_string())?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("Cannot stop command tree: {status}"))
        }
    }
}

async fn run(
    script: &str,
    payload: &Value,
    directory: &Path,
    logs: &Logs,
    cancel: &AtomicBool,
) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        return Err("Command cancelled".into());
    }
    // A file supplies JSON and EOF even when the command never reads stdin.
    let mut input = tempfile::tempfile().map_err(|e| e.to_string())?;
    serde_json::to_writer(&mut input, payload).map_err(|e| e.to_string())?;
    input.write_all(b"\n").map_err(|e| e.to_string())?;
    input.rewind().map_err(|e| e.to_string())?;
    let mut child = shell(script)?
        .current_dir(directory)
        .stdin(Stdio::from(input))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("Cannot start command: {e}"))?;
    let pid = child.id().ok_or("Command has no process ID")?;
    #[cfg(unix)]
    let mut group = ProcessGroup(Some(pid));
    let mut readers = JoinSet::new();
    let stdout = child.stdout.take().ok_or("Missing command stdout")?;
    let stderr = child.stderr.take().ok_or("Missing command stderr")?;
    readers.spawn(async move { ("stdout", capture(stdout).await) });
    readers.spawn(async move { ("stderr", capture(stderr).await) });
    logs.write(
        "conversion",
        format!("Post-processing: {}", payload["output_path"]),
    );
    let mut cancelled = false;
    let status = loop {
        tokio::select! {
            biased;
            result = child.wait() => break result.map_err(|e| e.to_string()),
            _ = tokio::time::sleep(Duration::from_millis(25)) => {
                if cancel.load(Ordering::Relaxed) {
                    cancelled = true;
                    if let Err(error) = terminate(pid).await {
                        // The shell may have finished between the cancellation check and taskkill.
                        if child.try_wait().map_err(|e| e.to_string())?.is_none() { return Err(error); }
                    }
                    break child.wait().await.map_err(|e| e.to_string());
                }
            }
        }
    };
    let status = status?;
    #[cfg(unix)]
    {
        group.0 = None;
    }
    let mut error_tail = String::new();
    let drained = tokio::time::timeout(Duration::from_secs(1), async {
        while let Some(result) = readers.join_next().await {
            let (stream, data) = result.map_err(|e| e.to_string())?;
            let data = data.map_err(|e| e.to_string())?;
            if stream == "stderr" {
                error_tail =
                    String::from_utf8_lossy(&data.bytes[data.bytes.len().saturating_sub(2048)..])
                        .trim()
                        .into();
            }
            if !data.bytes.is_empty() {
                let suffix = if data.truncated {
                    " (last 256 KiB)"
                } else {
                    ""
                };
                logs.write(
                    "conversion",
                    format!(
                        "Post-processing {stream}{suffix}:\n{}",
                        String::from_utf8_lossy(&data.bytes)
                    ),
                );
            }
        }
        Ok::<_, String>(())
    })
    .await;
    if cancelled {
        return Err("Command cancelled".into());
    }
    drained.map_err(|_| {
        "Command exited but its output streams did not close; wait for all child processes"
    })??;
    if !status.success() {
        return Err(format!("Command {status}: {error_tail}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::DngLook;
    use std::{fs, sync::Arc};

    fn published() -> (
        tempfile::TempDir,
        std::path::PathBuf,
        std::path::PathBuf,
        ExifTool,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("source 色 $(not-a-command).X3F");
        let output = dir.path().join("photo.dng");
        let staging = dir.path().join("staged.dng");
        fs::write(&input, b"original X3F").unwrap();
        fs::write(&staging, b"converted DNG").unwrap();
        crate::conversion::publish(&staging, &output, false).unwrap();
        // Metadata is optional: these unit tests do not need an ExifTool installation.
        let exif = ExifTool::new(
            dir.path().join("no-exiftool"),
            Arc::new(Logs::new(dir.path().join("logs"))),
        );
        (dir, input, output, exif)
    }

    fn script(directory: &Path, unix: &str, windows: &str) -> String {
        #[cfg(unix)]
        {
            let _ = windows;
            fs::write(directory.join("hook.sh"), unix).unwrap();
            "/bin/sh hook.sh".into()
        }
        #[cfg(windows)]
        {
            let _ = unix;
            fs::write(directory.join("hook.ps1"), format!("\u{feff}{windows}")).unwrap();
            let powershell = system_tool("WindowsPowerShell")
                .unwrap()
                .join("v1.0/powershell.exe");
            format!(
                "\"{}\" -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File hook.ps1",
                powershell.display()
            )
        }
    }

    #[test]
    fn request_contains_effective_options_and_optional_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("色 source.X3F");
        let output = dir.path().join("色 source.dng");
        let settings = BatchSettings {
            dng_look: DngLook::MerrillSpp10,
            compress: true,
            denoise_intensity: 3,
            dng_highlight_recovery: true,
            color_profile: ColorProfile::ProPhotoRgb,
            ..Default::default()
        };
        let value = request(
            &input,
            &output,
            &settings,
            dir.path(),
            json!({"Aperture": 5.6}),
        );
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["input_path"], input.to_str().unwrap());
        assert_eq!(value["output_path"], output.to_str().unwrap());
        assert_eq!(value["output_format"], "dng");
        assert_eq!(value["metadata"], json!({"Aperture": 5.6}));
        assert_eq!(
            value["conversion_settings"],
            json!({
                "compress": true, "denoise_intensity": 3, "highlight_recovery": true,
                "color_profile": "ProPhotoRGB",
                "look_profile_path": dir.path().join("profiles/merrill_spp_1_0.dcp")
            })
        );
        assert!(request(
            &input,
            &output,
            &BatchSettings::default(),
            dir.path(),
            json!({})
        )["conversion_settings"]["look_profile_path"]
            .is_null());
    }

    #[tokio::test]
    async fn disabled_and_non_dng_hooks_do_not_run() {
        let (dir, input, output, exif) = published();
        let mut settings = BatchSettings::default();
        for format in [
            OutputFormat::Dng,
            OutputFormat::Tiff,
            OutputFormat::Jpeg,
            OutputFormat::RenderedJpeg,
        ] {
            settings.output_format = format;
            settings.dng_post_processing_command = if format == OutputFormat::Dng {
                " \n\t "
            } else {
                "exit 19"
            }
            .into();
            apply(
                &input,
                &output,
                &settings,
                dir.path(),
                &exif,
                &AtomicBool::new(false),
            )
            .await
            .unwrap();
        }
        assert_eq!(fs::read(&output).unwrap(), b"converted DNG");
    }

    #[tokio::test]
    async fn command_receives_json_eof_and_the_final_directory() {
        let (dir, input, output, exif) = published();
        let command = script(dir.path(),
            "test -f photo.dng || exit 20\ncat > request.json\nprintf 'out'\nprintf 'err' >&2\n",
            "$s = [Console]::OpenStandardInput()\n$f = [IO.File]::Create('request.json')\n$s.CopyTo($f)\n$f.Dispose()\nif (!(Test-Path photo.dng)) { exit 20 }\n[Console]::Out.Write('out')\n[Console]::Error.Write('err')\n");
        let settings = BatchSettings {
            dng_post_processing_command: command,
            ..Default::default()
        };
        apply(
            &input,
            &output,
            &settings,
            dir.path(),
            &exif,
            &AtomicBool::new(false),
        )
        .await
        .unwrap();
        let bytes = fs::read(dir.path().join("request.json")).unwrap();
        assert!(bytes.ends_with(b"\n"));
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["input_path"], input.to_str().unwrap());
        assert_eq!(value["output_path"], output.to_str().unwrap());
        assert_eq!(value["metadata"], json!({}));
        let log = fs::read_to_string(exif.logs.dir.join("conversion.log")).unwrap();
        assert!(log.contains("stdout:\nout"));
        assert!(log.contains("stderr:\nerr"));
        assert_eq!(fs::read(&input).unwrap(), b"original X3F");
    }

    #[tokio::test]
    async fn failures_keep_published_output_and_missing_output_is_reported() {
        let (dir, input, output, exif) = published();
        let mut settings = BatchSettings {
            dng_post_processing_command: "exit 31".into(),
            ..Default::default()
        };
        let error = apply(
            &input,
            &output,
            &settings,
            dir.path(),
            &exif,
            &AtomicBool::new(false),
        )
        .await
        .unwrap_err();
        assert!(error.contains("31"), "{error}");
        assert_eq!(fs::read(&output).unwrap(), b"converted DNG");
        settings.dng_post_processing_command =
            script(dir.path(), "rm photo.dng", "Remove-Item photo.dng");
        let error = apply(
            &input,
            &output,
            &settings,
            dir.path(),
            &exif,
            &AtomicBool::new(false),
        )
        .await
        .unwrap_err();
        assert!(error.contains("Output missing"), "{error}");
        assert_eq!(fs::read(&input).unwrap(), b"original X3F");
    }

    #[tokio::test]
    async fn output_is_bounded_and_stdin_need_not_be_consumed() {
        let dir = tempfile::tempdir().unwrap();
        let logs = Logs::new(dir.path().join("logs"));
        let command = script(
            dir.path(),
            "printf '%0300000d' 1\nprintf '%0300000d' 2 >&2",
            "[Console]::Out.Write('a' * 300000)\n[Console]::Error.Write('b' * 300000)",
        );
        let payload = json!({"padding": "x".repeat(LOG_LIMIT * 2)});
        tokio::time::timeout(
            Duration::from_secs(15),
            run(
                &command,
                &payload,
                dir.path(),
                &logs,
                &AtomicBool::new(false),
            ),
        )
        .await
        .unwrap()
        .unwrap();
        let log = fs::read_to_string(logs.dir.join("conversion.log")).unwrap();
        assert!(log.contains("stdout (last 256 KiB)"));
        assert!(log.contains("stderr (last 256 KiB)"));
        assert!(log.len() < 2 * LOG_LIMIT + 1024);
    }

    #[tokio::test]
    async fn cancellation_stops_foreground_children_and_keeps_the_dng() {
        let (dir, input, output, exif) = published();
        fs::write(
            dir.path().join("child.ps1"),
            "Set-Content ready yes\nStart-Sleep -Seconds 4\nSet-Content late leaked",
        )
        .unwrap();
        let command = script(dir.path(),
            "trap '' TERM\n(sleep 4; printf leaked > late) &\nprintf ready > ready\nwait",
            "$child = Start-Process (Join-Path $PSHOME 'powershell.exe') -NoNewWindow -PassThru -ArgumentList '-NoProfile','-NonInteractive','-ExecutionPolicy','Bypass','-File','child.ps1'\n$child.WaitForExit()");
        let settings = BatchSettings {
            dng_post_processing_command: command,
            ..Default::default()
        };
        let cancel = AtomicBool::new(false);
        let ready = dir.path().join("ready");
        let (result, ()) = tokio::join!(
            apply(&input, &output, &settings, dir.path(), &exif, &cancel),
            async {
                for _ in 0..200 {
                    if ready.exists() {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                cancel.store(true, Ordering::Relaxed);
            }
        );
        assert!(ready.exists(), "Command never became ready");
        assert!(result.unwrap_err().contains("cancelled"));
        tokio::time::sleep(Duration::from_millis(4200)).await;
        assert!(!dir.path().join("late").exists());
        assert_eq!(fs::read(&output).unwrap(), b"converted DNG");
        assert_eq!(fs::read(&input).unwrap(), b"original X3F");
    }
}
