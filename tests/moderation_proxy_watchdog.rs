//! Spawn/kill only a fixture owner, with all OS proxy calls redirected to a fake.
#![cfg(target_os = "macos")]
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
#[test]
fn abrupt_owner_exit_restores_proxy_and_normal_disarm_preserves_later_settings() {
    for mode in ["kill", "disarm"] {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        fs::write(p.join("networksetup"),r#"#!/usr/bin/python3
import os,json,sys
p=os.environ['LIVE_ARCHIVE_PROXY_FIXTURE'];a=sys.argv[1:];v=json.load(open(p));secure='secure' in a[0];k='secure_web' if secure else 'web'
if a[0].startswith('-get'):
 x=v[k];print('Enabled: '+('Yes' if x else 'No'));print('Server: '+(x['host'] if x else ''));print('Port: '+str(x['port'] if x else 0))
elif a[0] in ['-setwebproxy','-setsecurewebproxy']:v[k]={'host':a[2],'port':int(a[3])}
elif a[0] in ['-setwebproxystate','-setsecurewebproxystate']:
 if a[2]=='off':v[k]=None
else:raise Exception('unexpected fake command')
if not a[0].startswith('-get'):json.dump(v,open(p,'w'))
"#).unwrap();
        fs::set_permissions(p.join("networksetup"), fs::Permissions::from_mode(0o755)).unwrap();
        let state = p.join("proxy.json");
        let local = json!({"host":"127.0.0.1","port":2024});
        fs::write(&state, json!({"web":local,"secure_web":local}).to_string()).unwrap();
        let descriptor = json!({"service":"Fixture Wi-Fi","original":{"web":{"host":"previous-proxy","port":8080},"secure_web":null},"installed":local});
        let owner = p.join("owner.py");
        fs::write(&owner,r#"import subprocess,os,json,sys,time,pathlib
p=pathlib.Path(sys.argv[2]);child=subprocess.Popen([sys.argv[1],'moderation','proxy-watchdog'],stdin=subprocess.PIPE,stdout=subprocess.PIPE,text=True,start_new_session=True)
child.stdin.write(sys.argv[3]+'\n');child.stdin.flush();assert child.stdout.readline().strip()=='ready'
(p/'watchdog-pid').write_text(str(child.pid));(p/'ready').write_text('ready')
if sys.argv[4]=='disarm':
 json.dump({'web':{'host':'later-user-proxy','port':7890},'secure_web':None},open(p/'proxy.json','w'))
 child.stdin.write('restored\n');child.stdin.close();assert child.wait(timeout=10)==0
else:time.sleep(60)
"#).unwrap();
        let mut child = Command::new(if cfg!(windows) { "python" } else { "python3" })
            .args(["-X", "utf8"])
            .arg(&owner)
            .arg(env!("CARGO_BIN_EXE_live-archive-core"))
            .arg(p)
            .arg(descriptor.to_string())
            .arg(mode)
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    p.display(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            )
            .env("LIVE_ARCHIVE_PROXY_FIXTURE", &state)
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        while !p.join("ready").exists() {
            if child.try_wait().unwrap().is_some() {
                panic!("owner failed to start watchdog");
            }
            if Instant::now() > deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("watchdog readiness timeout");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        if mode == "kill" {
            child.kill().unwrap();
        }
        let status = child.wait().unwrap();
        if mode == "disarm" {
            assert!(status.success());
        }
        let expected = if mode == "kill" {
            json!({"web":{"host":"previous-proxy","port":8080},"secure_web":null})
        } else {
            json!({"web":{"host":"later-user-proxy","port":7890},"secure_web":null})
        };
        loop {
            let value = fs::read_to_string(&state)
                .ok()
                .and_then(|s| serde_json::from_str::<Value>(&s).ok());
            if value.as_ref() == Some(&expected) {
                break;
            }
            if Instant::now() > deadline {
                if let Ok(pid) = fs::read_to_string(p.join("watchdog-pid"))
                    .unwrap_or_default()
                    .parse::<i32>()
                {
                    unsafe {
                        libc::kill(pid, libc::SIGKILL);
                    }
                }
                panic!("proxy recovery failed: {value:?}");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
