use crate::credential_store::host_password_get;
use crate::models::Host;
use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostStaticInfo {
    pub system_name: Option<String>,
    pub kernel: Option<String>,
    pub arch: Option<String>,
    pub cpu_model: Option<String>,
    pub cpu_cores: Option<u32>,
    pub mem_total_kb: Option<u64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostLiveProcess {
    pub command: String,
    pub cpu_percent: f64,
    pub mem_percent: f64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostLiveInfo {
    pub cpu_percent: Option<f64>,
    pub cpu_user_percent: Option<f64>,
    pub cpu_system_percent: Option<f64>,
    pub cpu_iowait_percent: Option<f64>,
    pub cpu_idle_percent: Option<f64>,
    pub cpu_cores: Option<u32>,
    pub uptime_seconds: Option<u64>,
    pub mem_total_kb: Option<u64>,
    pub mem_used_kb: Option<u64>,
    pub mem_free_kb: Option<u64>,
    pub mem_page_cache_kb: Option<u64>,
    pub load_1: Option<f64>,
    pub load_5: Option<f64>,
    pub load_15: Option<f64>,
    pub disk_root_total_kb: Option<u64>,
    pub disk_root_used_kb: Option<u64>,
    pub processes: Vec<HostLiveProcess>,
}

fn shell_quote(input: &str) -> String {
    format!("'{}'", input.replace('\'', "'\\''"))
}

fn maybe_text(value: Option<&String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty() && v != "unknown")
}

fn parse_u32(value: Option<&String>) -> Option<u32> {
    value.and_then(|v| v.trim().parse::<u32>().ok())
}

fn parse_u64(value: Option<&String>) -> Option<u64> {
    value.and_then(|v| v.trim().parse::<u64>().ok())
}

fn parse_f64(value: Option<&String>) -> Option<f64> {
    value.and_then(|v| v.trim().parse::<f64>().ok())
}

fn parse_kv(stdout: &str) -> (HashMap<String, String>, Vec<String>) {
    let mut kv = HashMap::new();
    let mut proc_lines = Vec::new();
    for raw in stdout.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("proc=") {
            proc_lines.push(rest.to_string());
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            kv.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    (kv, proc_lines)
}

fn target_of(host: &Host) -> String {
    let user = host.user.trim();
    let hostname = host.hostname.trim();
    if user.is_empty() {
        hostname.to_string()
    } else {
        format!("{user}@{hostname}")
    }
}

fn create_askpass_script(password: &str) -> Result<PathBuf, String> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let path = std::env::temp_dir().join(format!("xtermius-askpass-{nonce}.sh"));
    let script = format!("#!/bin/sh\nprintf '%s\\n' {}\n", shell_quote(password));
    fs::write(&path, script).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        let mut perms = fs::metadata(&path).map_err(|e| e.to_string())?.permissions();
        perms.set_mode(0o700);
        fs::set_permissions(&path, perms).map_err(|e| e.to_string())?;
    }
    Ok(path)
}

fn run_probe(host: &Host, script: &str) -> Result<String, String> {
    let target = target_of(host);
    if target.trim().is_empty() {
        return Err("hostname is required".to_string());
    }

    let mut args: Vec<String> = vec![
        "-o".to_string(),
        "ConnectTimeout=8".to_string(),
        "-o".to_string(),
        "ConnectionAttempts=1".to_string(),
        "-o".to_string(),
        "StrictHostKeyChecking=accept-new".to_string(),
        "-o".to_string(),
        "ServerAliveInterval=10".to_string(),
        "-o".to_string(),
        "ServerAliveCountMax=1".to_string(),
    ];

    if host.port > 0 {
        args.push("-p".to_string());
        args.push(host.port.to_string());
    }
    if let Some(path) = host.identity_file.as_ref().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        args.push("-i".to_string());
        args.push(path.to_string());
    }
    if let Some(jump) = host.proxy_jump.as_ref().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        args.push("-J".to_string());
        args.push(jump.to_string());
    }

    let mut askpass_path: Option<PathBuf> = None;
    let mut cmd = Command::new("/usr/bin/ssh");
    let maybe_password = host_password_get(host.id.clone()).ok().flatten();
    if let Some(password) = maybe_password.as_ref().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        let p = create_askpass_script(password)?;
        askpass_path = Some(p.clone());
        args.push("-o".to_string());
        args.push("BatchMode=no".to_string());
        args.push("-o".to_string());
        args.push("NumberOfPasswordPrompts=1".to_string());
        cmd.env("DISPLAY", "xtermius:0");
        cmd.env("SSH_ASKPASS_REQUIRE", "force");
        cmd.env("SSH_ASKPASS", p.as_os_str());
    } else {
        args.push("-o".to_string());
        args.push("BatchMode=yes".to_string());
    }

    args.push(target);
    args.push("sh".to_string());
    args.push("-lc".to_string());
    args.push(script.to_string());

    let output = cmd
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| e.to_string())?;

    if let Some(path) = askpass_path {
        let _ = fs::remove_file(path);
    }

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let msg = if !stderr.is_empty() { stderr } else { stdout };
        return Err(if msg.is_empty() {
            format!("ssh exited with status {}", output.status)
        } else {
            msg
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn host_probe_static_impl(host: Host) -> Result<HostStaticInfo, String> {
    let script = r#"
set -eu
SYSTEM_NAME="$(hostnamectl --pretty 2>/dev/null || true)"
if [ -z "$SYSTEM_NAME" ] && [ -r /etc/os-release ]; then
  SYSTEM_NAME="$(awk -F= '/^PRETTY_NAME=/{gsub(/^"|"$/,"",$2);print $2;exit}' /etc/os-release 2>/dev/null || true)"
fi
if [ -z "$SYSTEM_NAME" ]; then
  SYSTEM_NAME="$(uname -s 2>/dev/null || true)"
fi
KERNEL="$(uname -sr 2>/dev/null || true)"
ARCH="$(uname -m 2>/dev/null || true)"
CPU_MODEL="$(awk -F: '/model name/{print $2;exit}' /proc/cpuinfo 2>/dev/null | sed 's/^ *//' || true)"
if [ -z "$CPU_MODEL" ]; then
  CPU_MODEL="$(sysctl -n machdep.cpu.brand_string 2>/dev/null || true)"
fi
CPU_CORES="$(getconf _NPROCESSORS_ONLN 2>/dev/null || nproc 2>/dev/null || sysctl -n hw.logicalcpu 2>/dev/null || true)"
MEM_TOTAL_KB="$(awk '/MemTotal:/ {print $2;exit}' /proc/meminfo 2>/dev/null || true)"
if [ -z "$MEM_TOTAL_KB" ]; then
  MEM_BYTES="$(sysctl -n hw.memsize 2>/dev/null || true)"
  if [ -n "$MEM_BYTES" ]; then
    MEM_TOTAL_KB="$((MEM_BYTES / 1024))"
  fi
fi
printf 'system_name=%s\n' "$SYSTEM_NAME"
printf 'kernel=%s\n' "$KERNEL"
printf 'arch=%s\n' "$ARCH"
printf 'cpu_model=%s\n' "$CPU_MODEL"
printf 'cpu_cores=%s\n' "$CPU_CORES"
printf 'mem_total_kb=%s\n' "$MEM_TOTAL_KB"
"#;

    let stdout = run_probe(&host, script)?;
    let (kv, _) = parse_kv(&stdout);
    Ok(HostStaticInfo {
        system_name: maybe_text(kv.get("system_name")),
        kernel: maybe_text(kv.get("kernel")),
        arch: maybe_text(kv.get("arch")),
        cpu_model: maybe_text(kv.get("cpu_model")),
        cpu_cores: parse_u32(kv.get("cpu_cores")),
        mem_total_kb: parse_u64(kv.get("mem_total_kb")),
    })
}

fn host_probe_live_impl(host: Host) -> Result<HostLiveInfo, String> {
    let script = r#"
set -eu
CPU_PERCENT=""
CPU_USER_PERCENT=""
CPU_SYSTEM_PERCENT=""
CPU_IOWAIT_PERCENT=""
CPU_IDLE_PERCENT=""
if [ -r /proc/stat ]; then
  LINE1="$(grep '^cpu ' /proc/stat || true)"
  CPU_CORES="$(grep -c '^cpu[0-9]' /proc/stat 2>/dev/null || true)"
  sleep 0.2
  LINE2="$(grep '^cpu ' /proc/stat || true)"
  if [ -n "$LINE1" ] && [ -n "$LINE2" ]; then
    CPU_ALL="$(awk -v A="$LINE1" -v B="$LINE2" 'BEGIN{
      split(A,a," "); split(B,b," ");
      user1=a[2]; nice1=a[3]; sys1=a[4]; idle1=a[5]; iow1=a[6];
      user2=b[2]; nice2=b[3]; sys2=b[4]; idle2=b[5]; iow2=b[6];
      total1=0; total2=0;
      for(i=2;i<=11;i++){ total1+=a[i]; total2+=b[i]; }
      dt=total2-total1;
      if(dt<=0){ print "||||"; exit; }
      user=((user2-user1)+(nice2-nice1))*100/dt;
      sys=(sys2-sys1)*100/dt;
      iow=(iow2-iow1)*100/dt;
      idle=(idle2-idle1)*100/dt;
      busy=100-idle;
      printf "%.1f|%.1f|%.1f|%.1f|%.1f", busy, user, sys, iow, idle;
    }')"
    CPU_PERCENT="$(printf '%s' "$CPU_ALL" | awk -F'|' '{print $1}')"
    CPU_USER_PERCENT="$(printf '%s' "$CPU_ALL" | awk -F'|' '{print $2}')"
    CPU_SYSTEM_PERCENT="$(printf '%s' "$CPU_ALL" | awk -F'|' '{print $3}')"
    CPU_IOWAIT_PERCENT="$(printf '%s' "$CPU_ALL" | awk -F'|' '{print $4}')"
    CPU_IDLE_PERCENT="$(printf '%s' "$CPU_ALL" | awk -F'|' '{print $5}')"
  fi
fi
if [ -z "$CPU_PERCENT" ]; then
  CPU_PERCENT="$(top -l 1 -n 0 2>/dev/null | awk -F'[:, ]+' '/CPU usage:/{print 100-$7;exit}' || true)"
fi
if [ -z "${CPU_CORES:-}" ]; then
  CPU_CORES="$(getconf _NPROCESSORS_ONLN 2>/dev/null || nproc 2>/dev/null || sysctl -n hw.logicalcpu 2>/dev/null || true)"
fi

UPTIME_SECONDS="$(awk '{print int($1)}' /proc/uptime 2>/dev/null || true)"
if [ -z "$UPTIME_SECONDS" ]; then
  UPTIME_SECONDS="$(sysctl -n kern.boottime 2>/dev/null | awk -F'[ ,}]+' '{for(i=1;i<=NF;i++) if($i=="sec"){print systime()-$(i+1); exit}}' || true)"
fi

MEM_TOTAL_KB="$(awk '/MemTotal:/ {print $2;exit}' /proc/meminfo 2>/dev/null || true)"
MEM_AVAIL_KB="$(awk '/MemAvailable:/ {print $2;exit}' /proc/meminfo 2>/dev/null || true)"
MEM_FREE_KB="$(awk '/MemFree:/ {print $2;exit}' /proc/meminfo 2>/dev/null || true)"
MEM_CACHE_KB="$(awk '/^Cached:/ {print $2;exit}' /proc/meminfo 2>/dev/null || true)"
MEM_USED_KB=""
if [ -n "$MEM_TOTAL_KB" ] && [ -n "$MEM_AVAIL_KB" ]; then
  MEM_USED_KB="$((MEM_TOTAL_KB - MEM_AVAIL_KB))"
fi
if [ -z "$MEM_TOTAL_KB" ]; then
  MEM_BYTES="$(sysctl -n hw.memsize 2>/dev/null || true)"
  if [ -n "$MEM_BYTES" ]; then
    MEM_TOTAL_KB="$((MEM_BYTES/1024))"
  fi
fi

LOAD_RAW="$(cat /proc/loadavg 2>/dev/null || uptime 2>/dev/null || true)"
LOAD_1="$(printf '%s\n' "$LOAD_RAW" | awk '{for(i=1;i<=NF;i++) if ($i ~ /^[0-9]+\.[0-9]+$/){print $i; exit}}')"
LOAD_5="$(printf '%s\n' "$LOAD_RAW" | awk '{c=0;for(i=1;i<=NF;i++) if ($i ~ /^[0-9]+\.[0-9]+$/){c++; if(c==2){print $i; exit}}}')"
LOAD_15="$(printf '%s\n' "$LOAD_RAW" | awk '{c=0;for(i=1;i<=NF;i++) if ($i ~ /^[0-9]+\.[0-9]+$/){c++; if(c==3){print $i; exit}}}')"

DISK_LINE="$(df -kP / 2>/dev/null | tail -n 1 || true)"
DISK_TOTAL_KB="$(printf '%s\n' "$DISK_LINE" | awk '{print $2}')"
DISK_USED_KB="$(printf '%s\n' "$DISK_LINE" | awk '{print $3}')"

printf 'cpu_percent=%s\n' "$CPU_PERCENT"
printf 'cpu_user_percent=%s\n' "$CPU_USER_PERCENT"
printf 'cpu_system_percent=%s\n' "$CPU_SYSTEM_PERCENT"
printf 'cpu_iowait_percent=%s\n' "$CPU_IOWAIT_PERCENT"
printf 'cpu_idle_percent=%s\n' "$CPU_IDLE_PERCENT"
printf 'cpu_cores=%s\n' "$CPU_CORES"
printf 'uptime_seconds=%s\n' "$UPTIME_SECONDS"
printf 'mem_total_kb=%s\n' "$MEM_TOTAL_KB"
printf 'mem_used_kb=%s\n' "$MEM_USED_KB"
printf 'mem_free_kb=%s\n' "$MEM_FREE_KB"
printf 'mem_page_cache_kb=%s\n' "$MEM_CACHE_KB"
printf 'load_1=%s\n' "$LOAD_1"
printf 'load_5=%s\n' "$LOAD_5"
printf 'load_15=%s\n' "$LOAD_15"
printf 'disk_root_total_kb=%s\n' "$DISK_TOTAL_KB"
printf 'disk_root_used_kb=%s\n' "$DISK_USED_KB"
ps -eo comm=,pcpu=,pmem= --sort=-pcpu 2>/dev/null | head -n 5 | while read -r cmd cpu mem; do
  [ -n "$cmd" ] || continue
  printf 'proc=%s|%s|%s\n' "$cmd" "$cpu" "$mem"
done
"#;

    let stdout = run_probe(&host, script)?;
    let (kv, proc_lines) = parse_kv(&stdout);
    let mut processes = Vec::new();
    for line in proc_lines {
        let mut parts = line.split('|');
        let command = parts.next().unwrap_or("").trim().to_string();
        let cpu_percent = parts
            .next()
            .unwrap_or("0")
            .trim()
            .parse::<f64>()
            .unwrap_or(0.0);
        let mem_percent = parts
            .next()
            .unwrap_or("0")
            .trim()
            .parse::<f64>()
            .unwrap_or(0.0);
        if command.is_empty() {
            continue;
        }
        processes.push(HostLiveProcess {
            command,
            cpu_percent,
            mem_percent,
        });
    }

    Ok(HostLiveInfo {
        cpu_percent: parse_f64(kv.get("cpu_percent")),
        cpu_user_percent: parse_f64(kv.get("cpu_user_percent")),
        cpu_system_percent: parse_f64(kv.get("cpu_system_percent")),
        cpu_iowait_percent: parse_f64(kv.get("cpu_iowait_percent")),
        cpu_idle_percent: parse_f64(kv.get("cpu_idle_percent")),
        cpu_cores: parse_u32(kv.get("cpu_cores")),
        uptime_seconds: parse_u64(kv.get("uptime_seconds")),
        mem_total_kb: parse_u64(kv.get("mem_total_kb")),
        mem_used_kb: parse_u64(kv.get("mem_used_kb")),
        mem_free_kb: parse_u64(kv.get("mem_free_kb")),
        mem_page_cache_kb: parse_u64(kv.get("mem_page_cache_kb")),
        load_1: parse_f64(kv.get("load_1")),
        load_5: parse_f64(kv.get("load_5")),
        load_15: parse_f64(kv.get("load_15")),
        disk_root_total_kb: parse_u64(kv.get("disk_root_total_kb")),
        disk_root_used_kb: parse_u64(kv.get("disk_root_used_kb")),
        processes,
    })
}

#[tauri::command]
pub async fn host_probe_static(host: Host) -> Result<HostStaticInfo, String> {
    tauri::async_runtime::spawn_blocking(move || host_probe_static_impl(host))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn host_probe_live(host: Host) -> Result<HostLiveInfo, String> {
    tauri::async_runtime::spawn_blocking(move || host_probe_live_impl(host))
        .await
        .map_err(|e| e.to_string())?
}
