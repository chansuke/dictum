//! Client for `dictum-oracle`: the Lean definitions in `formal/lean` as a test oracle.
//!
//! The proofs in `formal/lean` are about Lean definitions (`Dictum.eval`, the
//! reference parser). The differential tests connect them to the Rust code: they
//! send the same generated inputs to dictum and to the oracle and compare.
//!
//! The oracle is found at `$DICTUM_ORACLE`, else at the `lake build` output. If
//! `$DICTUM_ORACLE` is set, a missing oracle is an error, so `just drt` and CI
//! cannot skip the comparison by accident. If it is not set and nothing was built,
//! [`Oracle::locate`] says why and the tests report themselves as ignored.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

/// A running oracle. Queries are answered in order, one line each.
pub struct Oracle {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

/// A value of the Lean core fragment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoreVal {
    Int(i64),
    Bool(bool),
}

/// `Dictum.eval`'s result. `Internal` means the input was not well typed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoreResult {
    Ok(CoreVal),
    Overflow,
    DivisionByZero,
    Internal,
}

impl Oracle {
    /// Where the oracle is: `Ok(path)`, or `Err(reason)` if it is not available and
    /// the tests that need it should be ignored. Panics if `$DICTUM_ORACLE` names a
    /// file that does not exist.
    pub fn locate() -> Result<PathBuf, String> {
        if cfg!(target_os = "wasi") {
            return Err("WASI cannot start the oracle process; `just drt` runs these on the host".into());
        }
        if let Some(path) = std::env::var_os("DICTUM_ORACLE") {
            let path = PathBuf::from(path);
            assert!(path.is_file(), "DICTUM_ORACLE={} does not exist; run `just oracle`", path.display());
            return Ok(path);
        }
        let built = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../formal/lean/.lake/build/bin/dictum-oracle");
        if built.is_file() {
            Ok(built)
        } else {
            Err("dictum-oracle is not built; run `just drt` (needs Lean from `nix develop`)".into())
        }
    }

    /// Start the oracle at `path`.
    pub fn spawn(path: &std::path::Path) -> Oracle {
        let mut child = Command::new(path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap_or_else(|e| panic!("starting {}: {e}", path.display()));
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = BufReader::new(child.stdout.take().expect("piped stdout"));
        Oracle { child, stdin, stdout }
    }

    fn ask(&mut self, fields: &[&str]) -> Vec<String> {
        for f in fields {
            assert!(!f.contains(['\t', '\n']), "oracle query fields cannot contain tabs or newlines: {f:?}");
        }
        writeln!(self.stdin, "{}", fields.join("\t")).expect("writing to the oracle");
        self.stdin.flush().expect("writing to the oracle");
        let mut line = String::new();
        self.stdout.read_line(&mut line).expect("reading from the oracle");
        assert!(!line.is_empty(), "the oracle exited");
        let answer: Vec<String> = line.trim_end_matches('\n').split('\t').map(str::to_owned).collect();
        assert!(answer[0] != "bad", "the oracle could not read {fields:?}");
        answer
    }

    /// The reference parser: `Ok(s-expression)` or `Err(error code)`.
    pub fn parse(&mut self, src: &str) -> Result<String, String> {
        let answer = self.ask(&["parse", src]);
        match answer[0].as_str() {
            "ok" => Ok(answer[1].clone()),
            _ => Err(answer[1].clone()),
        }
    }

    /// `Dictum.eval env expr`, with `expr` in the oracle's s-expression syntax.
    pub fn eval(&mut self, env: &[CoreVal], expr: &str) -> CoreResult {
        let env: Vec<String> = env
            .iter()
            .map(|v| match v {
                CoreVal::Int(n) => format!("i:{n}"),
                CoreVal::Bool(b) => format!("b:{b}"),
            })
            .collect();
        let answer = self.ask(&["eval", &env.join(" "), expr]);
        match (answer[0].as_str(), answer[1].as_str()) {
            ("ok", "int") => CoreResult::Ok(CoreVal::Int(answer[2].parse().expect("an i64 from the oracle"))),
            ("ok", "bool") => CoreResult::Ok(CoreVal::Bool(answer[2] == "true")),
            (_, "overflow") => CoreResult::Overflow,
            (_, "divByZero") => CoreResult::DivisionByZero,
            _ => CoreResult::Internal,
        }
    }
}

/// A type of the Lean core fragment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoreTy {
    Int,
    Bool,
}

impl Oracle {
    /// `Dictum.infer types expr`: the type, or `None` if the rule is not well typed.
    pub fn check(&mut self, types: &[CoreTy], expr: &str) -> Option<CoreTy> {
        let types: Vec<&str> = types
            .iter()
            .map(|t| match t {
                CoreTy::Int => "int",
                CoreTy::Bool => "bool",
            })
            .collect();
        let answer = self.ask(&["check", &types.join(" "), expr]);
        match (answer[0].as_str(), answer.get(1).map(String::as_str)) {
            ("ok", Some("int")) => Some(CoreTy::Int),
            ("ok", Some("bool")) => Some(CoreTy::Bool),
            _ => None,
        }
    }
}

impl Drop for Oracle {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
