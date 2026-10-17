# CLI analytics security hardening

P028 hardens the local Rust-to-Python boundary used by `codex-meter estimate`. The command still
accepts AnalyticsRequest v1 and returns AnalyticsResult v1. Estimation remains local and
empirical; no provider service, quota endpoint, benchmark workload, or automatic quota acquisition
is invoked.

## Threat model

The CLI treats request JSON, SQLite contents, subprocess stdout and stderr, inherited interpreter
configuration, working-directory contents, and worker exit timing as untrusted. It validates the
request before starting Python and validates the complete worker result against the request scope
before rendering it.

The caller-selected `--python PATH` executable is trusted code. Selecting a malicious native
executable cannot be made safe by this integration: the executable runs with the caller's operating
system identity and may access files, processes, or services available to that identity. P028 does
not claim to sandbox it. Worker path checks establish which package files the application selected;
they are not cryptographic authentication of the interpreter or package contents.

## Trusted worker resolution

The development strategy resolves the Python source root relative to the compiled Rust crate's
manifest directory. Rust canonicalizes it and checks that the package initializer, integration
initializer, and worker file are regular files whose resolved paths remain below that root. The
fixed bootstrap checks those paths again before adding only that root to the application portion of
`sys.path`.

If the source root is absent, the bootstrap searches only the selected interpreter's standard
`purelib`/`platlib` locations, its standard site-package locations, and the virtual environment
site-packages directory derived from `sys.executable` and `pyvenv.cfg`. Exactly one matching
installation must exist. Missing or ambiguous roots fail with
`python_worker_resolution_failed`. The current working directory and `PYTHONPATH` are never worker
identity sources.

The executable is invoked with `-I -S -c` and a fixed Rust-owned bootstrap. CPython's `-I` implies
environment isolation (`-E`), disables the user site (`-s`), and enables safe-path behavior;
`-S` disables normal `site` initialization. The bootstrap uses only standard-library path
calculation, validates the selected package root, then runs the narrow worker entry point. It does
not evaluate request data as Python or as an import path. The source-tree and installed-package
strategies use the same bootstrap. An installed package is located from interpreter configuration,
not from a successful import or the working directory.

## Interpreter and environment boundary

`--python PATH` remains supported and is passed as the executable to `std::process::Command` with
discrete arguments. There is no shell command string, shell interpolation, executable fallback,
or retry. The selected executable must support the isolation flags and must be trusted by the
caller.

The child environment is cleared and rebuilt from this allowlist:

- `PATH` for resolving the default `python` executable;
- `PATHEXT` on Windows executable lookup;
- `SystemRoot` and `WINDIR` for Windows process startup;
- `TEMP`, `TMP`, and `TMPDIR` for ordinary temporary-file behavior.

No credentials or provider tokens are deliberately forwarded. The Python runtime does not depend
on `PYTHONPATH`, `PYTHONHOME`, `PYTHONSTARTUP`, `PYTHONINSPECT`, user-site configuration, or
ambient startup files. The selected native interpreter still has the caller's operating-system
permissions, so the environment policy is not a filesystem or process sandbox.

## Process protocol and bounds

The local process protocol remains version `1.0.0`. Standard input contains one AnalyticsRequest v1
document, capped at 64 KiB. The worker reads through EOF before analysis. On successful completion
it writes one fixed `codex_meter_worker_v1 input_complete` marker to stderr; Rust requires the exact
marker as an input-completion acknowledgement. This marker detects an incomplete or incompatible
worker exchange, but it is not authentication. Standard output is capped at 2 MiB and must contain
exactly one complete UTF-8 JSON document. Retained stderr is capped at 8 KiB and is never displayed
verbatim. Extra banners, documents, invalid UTF-8, unknown fields, duplicate fields, forbidden
nulls, and trailing non-whitespace text are rejected.

The default 60-second deadline starts before process launch and covers request writing, output and
diagnostic collection, child exit, and cleanup. The supervisor reserves up to 250 ms of that
deadline for termination and pipe cleanup. Output overflow stops the corresponding reader and
terminates the process tree; it does not continue consuming unbounded output. If the direct child
or its pipes do not close by the deadline, the invocation fails with a process cleanup error.
Limits are checked for zero and out-of-range values before launch. Rust's synchronous operating
system process-creation call cannot itself be interrupted; its elapsed time is included and checked
as soon as it returns.

On Linux and macOS, the worker starts in a new process group. Timeout, output overflow, or inherited
pipe handles after direct-child exit trigger a group kill; Rust also reaps the direct child. On
Windows, the child is assigned to a narrowly scoped Job Object configured to terminate members when
the job handle closes. If job setup or assignment fails, the invocation fails closed. A child that
escapes a Unix process group, or a child created in the brief interval before Windows job
assignment, is outside the guarantees of this small process runner. If escaped descendants keep a
pipe open, Rust returns a bounded cleanup failure and detaches only the blocked reader handle; it
does not return a successful result. If the OS has not reaped a killed direct child by the cleanup
deadline, Rust transfers its handle to a background reaper before returning the cleanup failure.

The request writer reports broken-pipe and write failures. A zero exit code alone is not success:
Rust also requires completed input delivery, closed pipes, bounded output, the exact worker
acknowledgement, valid JSON, and semantic result validation. No automatic retry occurs.

## Contract and scope validation

Rust's typed Serde request and result structures reject duplicate top-level and nested fields,
unknown fields, and explicit nulls for optional values. Tests demonstrate typed deserialization's
duplicate rejection on both public JSON boundaries; decoding does not first collapse objects into
a generic JSON map. Python uses `object_pairs_hook` to reject duplicate request fields. A focused
compatibility corpus covers AnalyticsRequest v1 and LocalExecutionContext protocol v1 behavior.

Before output, Rust binds the result to the request ID, analysis type, schema and estimator versions,
exact configuration identity, requested meter order and coverage, generated-at timestamp, status,
reason codes, warnings, and sample accounting. Sufficient estimates require an explicit manual
position for that meter. Rust independently verifies the returned used and remaining amounts using
`full_capacity * (manual_percent / 100)` and `full_capacity - expected_used`. It uses the P020
relative tolerance (`1e-12`, with zero absolute tolerance); 0% and 100% are valid. An insufficient
estimate may omit its manual position and its numerical capacities.

The worker reads history through P017's SQLite reader with URI `mode=ro` and `PRAGMA query_only =
ON`. It runs no migrations, writes, repairs, checkpoints, or implicit database creation. Corrupt or
unsupported storage fails with a fixed category. Tests compare synthetic database bytes and state
before and after analytics. SQLite journal/WAL sidecar behavior is platform-dependent; the tests do
not claim to cover every concurrent-writer or filesystem implementation.

Relative database paths are resolved against the CLI working directory; absolute paths, spaces,
Unicode, and Windows paths are passed as discrete process arguments. Rust does not canonicalize the
database path. Python opens the resulting path read-only. A file replacement between CLI validation
and SQLite open remains a filesystem race; eliminating it would require a larger storage-interface
change.

## Privacy-safe output

Worker errors are converted to fixed codes and explanations. Tracebacks, raw stdout, unrestricted
stderr, request JSON, Observation JSON, environment values, source contents, credentials, and
provider authentication details are not included in CLI diagnostics. Synthetic secret, prompt,
response, and source markers are exercised in rejected-input tests. Error paths do not trigger
provider calls or benchmark execution.

Successful JSON output is reserialized from the validated AnalyticsResult v1 model. Human output
does not print source records and identifies its values as Codex Meter empirical estimates, not
official provider quota allowances. An insufficient-evidence result preserves sample accounting
and reason codes and does not substitute zero for an unavailable capacity.

## Cross-platform verification and limitations

CI runs Rust process and CLI tests on Ubuntu, macOS, and Windows. The local P028 test suite includes
source-tree loading, installed-package loading in a temporary virtual environment with a path
containing spaces, CLI database paths containing spaces and Unicode, working-directory and Python
environment shadowing, timeout, output limits, descendant-held pipes, read-only storage, duplicate
JSON fields, privacy canaries, result scope, and manual-position arithmetic. These checks use
synthetic data and do not access provider services.

The process runner cannot constrain a malicious interpreter, authenticate files cryptographically,
eliminate filesystem replacement races, or terminate a Unix descendant that deliberately escapes
its process group. Windows Job Object assignment occurs immediately after spawn rather than before
the first instruction. An unreapable operating-system process or pipe returns a fixed cleanup error;
that rare OS-level failure is not converted into a success result.

## Inputs for P029

P029 may assume:

- P027's current-capacity CLI remains functional.
- Rust/Python transport uses the hardened worker-resolution policy.
- Environment-dependent module shadowing is prevented within the documented trust model.
- Process invocation has bounded output, diagnostics, and lifecycle handling.
- Request/result validation is strict and cross-language semantics are tested.
- Explicit current-meter percentages are verified against returned arithmetic.
- SQLite analytical access remains read-only.
- Errors do not expose raw telemetry or sensitive payloads.
- No provider execution occurs.
- No automatic quota acquisition is claimed.
- Benchmark production execution remains disabled.

P029 focuses on:

**Cross-platform resilience, installation/runtime compatibility, and operational hardening.**
