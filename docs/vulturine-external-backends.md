# Vulturine External Servo/Ladybird Bridges

Vulturine stays lightweight by default: `Software` backend is built-in, while `Servo` and `Ladybird` can be plugged in as external processes.

## Zero-config behavior

Vulturine now attempts auto-discovery first:

- searches common executable names in `PATH`
- tries common CLI template variants per backend
- falls back to `Software` if all attempts fail

Optional simple bin env vars:

- `VULTURINE_SERVO_BIN`
- `VULTURINE_LADYBIRD_BIN`

If these are set, Vulturine auto-generates command templates for that binary.

## Runtime backend selection

- UI: top bar dropdown (`Software`, `Servo`, `Ladybird`)
- env var: `VULTURINE_BACKEND=servo|ladybird|software`

## Bridge command env vars

- `VULTURINE_SERVO_CMD_JSON`
- `VULTURINE_LADYBIRD_CMD_JSON`

Each value must be a JSON array of command tokens.

Supported placeholders in each token:

- `{url}`
- `{width}`
- `{height}`
- `{out}` (path to temporary PNG output file)

If `{out}` is used, the external process must write a PNG there.
If `{out}` is not used, the process must write PNG bytes to stdout.

## Example

```powershell
$env:VULTURINE_BACKEND = "servo"
$env:VULTURINE_SERVO_BIN = "C:\\tools\\servo.exe"
```

```bash
export VULTURINE_BACKEND=ladybird
export VULTURINE_LADYBIRD_BIN=/usr/local/bin/ladybird
```

If a bridge fails or is not configured, Vulturine automatically falls back to `Software`.
