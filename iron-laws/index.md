# Iron Laws

Non-negotiable workflow rules. Override only via ADR.

| ID | Law |
|---|---|
| MUST-1  | Never edit `main` directly. |
| MUST-3  | Quoted exit code + test count + build log before any "done" claim. |
| MUST-12 | All hooks default `exit 0`. Use `--strict` to escalate to `exit 2`. |
| MUST-13 | `.dev-kit/.active-hooks.json` is the SSOT for hook state. |
| MUST-21 | 0-arg UX for slash commands; branching via `when_to_use` auto-match. |
| MUST-25 | No over-engineering — defaults handle 80%. ADR required to add a feature. |
| MUST-29 | HOTL: deterministic steps auto, single Y/n for opt-in CI install. |
| MUST-36 | Per-step sub-agent delegation + self-fix loop in `/dev-kit:build`. |
| MUST-48 | `tdd-guard` only active when `lib/methodology/tdd.py` is loaded. |
| MUST-L1 | No refactor without a regression test. |
| MUST-L2 | No fix proposal before Phase 1 (reproduce) completes. |
| MUST-L3 | No "done" without verification (quoted exit codes). |
| MUST-L4 | Root-cause-first debugging. |
| MUST-NOT-13 | No extra option prompts beyond hidden flags. |

See `hooks/index.md` for how each law is enforced per stage.
