# Hooks

Per-stage hook matrix (SSOT = `.dev-kit/.active-hooks.json`).

| Hook           | Stages ON                                | Default action |
|----------------|------------------------------------------|----------------|
| `tdd-guard`    | build                                    | exit 0 (warn on prod-without-test) |
| `bash-guard`   | build                                    | exit 0 (block `rm -rf`, destructive git) |
| `secret-scan`  | build / review / security                | exit 0 (warn on credential patterns) |
| `slop-detector`| build / review / security                | exit 0 (warn on KO+EN banned phrases) |
| `stop-verify`  | plan / design / build / review / security / ship | exit 0 (require quoted exit codes in "done" claims) |

## Bootstrap stage
All hooks OFF. `secret-scan` in read-only mode. No blocking during initial setup.

## Strict mode
Pass `--strict` to escalate all hooks to `exit 2` (block on violation).

## Disable individual hooks
```sh
DEV_KIT_HOOK_OFF=tdd-guard,bash-guard
```

## Reading the matrix
```sh
jq '.matrix.build' .dev-kit/.active-hooks.json
```
