//! The bridge to Node for TypeScript config and custom rules (ADR 0008).
//!
//! The host is one Node child process, started on first use. Requests and
//! responses are single JSON lines on the child's stdin and stdout.

use std::env;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{json, Value};

use crate::config::PackDescription;
use crate::error::{Error, Result};
use crate::model::{Diagnostic, FixResult};
use crate::rule::{Change, FixOutcome, Options};

/// Version of the JSON-lines protocol. The host must report the same number.
pub const PROTOCOL_VERSION: u64 = 1;

/// Overrides the Node executable. Default: `node` on `PATH`.
pub const NODE_ENV: &str = "FSCALE_NODE";
/// Overrides the path of the host script.
pub const HOST_ENV: &str = "FSCALE_NODE_HOST";

struct Running {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

/// A lazily started Node host for one repo root.
pub struct NodeHost {
    root: PathBuf,
    running: Option<Running>,
}

/// Looks for the host script next to the binary: this repo's dev layout
/// (`target/<profile>/fscale` beside `packages/cli`) and an npm install
/// (`node_modules/firescale` beside the platform package).
fn host_script_near_exe() -> Option<PathBuf> {
    let exe = env::current_exe().ok()?;
    exe.ancestors().skip(1).take(5).find_map(|dir| {
        [
            dir.join("packages")
                .join("cli")
                .join("dist")
                .join("host.js"),
            dir.join("firescale").join("dist").join("host.js"),
            dir.join("node_modules")
                .join("firescale")
                .join("dist")
                .join("host.js"),
        ]
        .into_iter()
        .find(|candidate| candidate.is_file())
    })
}

impl NodeHost {
    #[must_use]
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            running: None,
        }
    }

    /// True once a Node process has been started for this run.
    #[must_use]
    pub fn started(&self) -> bool {
        self.running.is_some()
    }

    fn start(&mut self, purpose: &str, alternative: &str) -> Result<&mut Running> {
        if self.running.is_none() {
            let node = env::var_os(NODE_ENV).unwrap_or_else(|| "node".into());
            let mut command = Command::new(&node);
            match env::var_os(HOST_ENV)
                .map(PathBuf::from)
                .or_else(host_script_near_exe)
            {
                Some(script) => {
                    command.arg(script);
                }
                // Let Node resolve the host from the repo's own node_modules.
                None => {
                    command.args(["--input-type=module", "--eval", "import 'firescale/host';"]);
                }
            }
            let mut child = command
                .current_dir(&self.root)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()
                .map_err(|error| {
                    Error::user(format!(
                        "Could not start Node (\"{}\": {error}), which is needed to {purpose}. Install Node 22.18 or newer and make sure it is on your PATH, or {alternative}.",
                        node.to_string_lossy()
                    ))
                })?;
            let stdin = child.stdin.take();
            let stdout = child.stdout.take().map(BufReader::new);
            let Some(stdout) = stdout else {
                return Err(Error::internal(
                    "The Node host has no stdout pipe. Please report this.",
                ));
            };
            self.running = Some(Running {
                child,
                stdin,
                stdout,
                next_id: 1,
            });
            let hello = self.request("hello", json!({}))?;
            let version = hello.get("protocolVersion").and_then(Value::as_u64);
            if version != Some(PROTOCOL_VERSION) {
                return Err(Error::user(format!(
                    "The firescale npm package in this repo speaks bridge protocol {} but this fscale binary speaks {PROTOCOL_VERSION}. Install matching versions of fscale and the firescale package.",
                    version.map_or_else(|| "unknown".to_owned(), |v| v.to_string())
                )));
            }
        }
        self.running
            .as_mut()
            .ok_or_else(|| Error::internal("The Node host is not running. Please report this."))
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let gone = || {
            Error::user(
                "The Node host stopped unexpectedly. Check the output above for an error from Node, and make sure the firescale npm package is installed in this repo (for example `pnpm add -D firescale`).",
            )
        };
        let running = self.running.as_mut().ok_or_else(gone)?;
        let id = running.next_id;
        running.next_id += 1;
        let line = json!({ "id": id, "method": method, "params": params }).to_string();
        let stdin = running.stdin.as_mut().ok_or_else(gone)?;
        writeln!(stdin, "{line}")
            .and_then(|()| stdin.flush())
            .map_err(|_| gone())?;

        let mut response = String::new();
        let read = running
            .stdout
            .read_line(&mut response)
            .map_err(|_| gone())?;
        if read == 0 {
            return Err(gone());
        }
        let response: Value = serde_json::from_str(response.trim()).map_err(|_| {
            Error::user(format!(
                "Node printed text that is not a bridge message while handling \"{method}\". A config or custom rule is probably writing to process.stdout directly. Use console.log there (firescale sends it to stderr), then run again."
            ))
        })?;
        if response.get("id").and_then(Value::as_u64) != Some(id) {
            return Err(Error::internal(
                "The Node host answered out of order. Please report this.",
            ));
        }
        if let Some(error) = response.get("error") {
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            return Err(Error::user(message));
        }
        Ok(response.get("result").cloned().unwrap_or(Value::Null))
    }

    /// Imports a `.ts` config file from the repo root and returns its value.
    pub fn load_config(&mut self, file: &str) -> Result<Value> {
        self.start(&format!("load {file}"), "use firescale.config.json instead")?;
        self.request(
            "loadConfig",
            json!({ "root": self.root.to_string_lossy(), "file": file }),
        )
    }

    /// Imports custom packs and returns their rules' metadata.
    pub fn describe_packs(&mut self, specs: &[String]) -> Result<Vec<PackDescription>> {
        self.start(
            "load custom rule packs",
            "remove the custom packs from your firescale config",
        )?;
        let result = self.request(
            "describePacks",
            json!({ "root": self.root.to_string_lossy(), "packs": specs }),
        )?;
        serde_json::from_value(result).map_err(|error| {
            Error::user(format!(
                "A custom rule pack is not valid ({error}). Define rules with defineRule and packs with definePack from the firescale SDK."
            ))
        })
    }

    /// Runs one custom rule's check and returns its diagnostics.
    pub fn check_rule(
        &mut self,
        pack: &str,
        rule_id: &str,
        options: &Options,
    ) -> Result<Vec<Diagnostic>> {
        self.start(
            "run custom rules",
            "remove the custom packs from your firescale config",
        )?;
        let result = self.request(
            "checkRule",
            json!({ "root": self.root.to_string_lossy(), "pack": pack, "ruleId": rule_id, "options": options }),
        )?;
        serde_json::from_value(result).map_err(|error| {
            Error::internal(format!(
                "Rule \"{rule_id}\" returned diagnostics in an unexpected shape ({error}). Please report this."
            ))
        })
    }

    /// Runs one custom rule's deterministic fix and returns what it changed.
    /// With `dry_run`, the SDK records the changes without applying them.
    pub fn fix_rule(
        &mut self,
        pack: &str,
        rule_id: &str,
        options: &Options,
        dry_run: bool,
    ) -> Result<FixOutcome> {
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Reply {
            #[serde(default)]
            changed_files: Vec<String>,
            #[serde(default)]
            notes: Vec<String>,
            #[serde(default)]
            changes: Vec<Change>,
        }
        self.start(
            "run custom rule fixes",
            "remove the custom packs from your firescale config",
        )?;
        let result = self.request(
            "fixRule",
            json!({
                "root": self.root.to_string_lossy(),
                "pack": pack,
                "ruleId": rule_id,
                "options": options,
                "dryRun": dry_run,
            }),
        )?;
        let reply: Reply = serde_json::from_value(result).map_err(|error| {
            Error::internal(format!(
                "The fix of rule \"{rule_id}\" returned an unexpected shape ({error}). Please report this."
            ))
        })?;
        Ok(FixOutcome {
            result: FixResult {
                changed_files: reply.changed_files,
                notes: reply.notes,
            },
            changes: reply.changes,
        })
    }
}

impl Drop for NodeHost {
    fn drop(&mut self) {
        if let Some(mut running) = self.running.take() {
            // Closing stdin tells the host to exit.
            drop(running.stdin.take());
            let _ = running.child.wait();
        }
    }
}
