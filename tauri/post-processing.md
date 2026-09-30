# DNG post-processing

Settings → **DNG post-processing → Command** stores an optional shell command.
Empty or whitespace-only disables it. Export review can override it for a batch;
starting that batch saves its settings. Commands and conversion options are
captured when the batch is prepared, so later settings changes do not affect it.

The command runs after conversion, EXIF copying, and publication under the final
DNG filename. It runs once per DNG, including explicit reconversions. TIFF and
JPEG exports do not invoke it. The selected DNG look is already embedded before
the command starts; look embedding does not require a command.

## Execution

- macOS/Linux: `/bin/sh -c <command>`.
- Windows: `cmd.exe /D /S /C <command>`. Use Windows command syntax, or explicitly
  launch another interpreter. The command is not automatically portable across OSes.
- Working directory: the final DNG's parent directory.
- Environment: inherited from the application. Shell startup files are not loaded;
  use absolute executable/script paths when the GUI's PATH is insufficient.
- Stdin: one UTF-8 JSON object, a newline, then EOF.
- Stdout/stderr: drained concurrently. The last 256 KiB of each is written to the
  conversion log after the command exits; truncation is marked. They are not a
  response protocol or a live terminal.

**Only configure commands you trust.** They run with your user permissions. No
filenames or metadata are inserted into the command string. Scripts must be
noninteractive and wait for their children. Background, detached, and elevated
processes are outside the cancellation contract. Output streams must close when
the command finishes; the app reports an error if they remain open afterward.

There is no automatic timeout or retry while the command is running. Each export
worker waits for its command, but different files can run concurrently. Choose
one export worker if your script requires serial execution.

Example command, using an interpreter you installed:

```sh
"/absolute/path/to/python" "/absolute/path/to/postprocess.py"
```

On Windows, use the corresponding quoted Windows paths. Neither Python, a preview
renderer, nor a specific processing script is bundled with this feature.

## Request

```json
{
  "schema_version": 1,
  "input_path": "/photos/example.X3F",
  "output_path": "/photos/example.dng",
  "output_format": "dng",
  "metadata": {
    "SourceFile": "/photos/example.X3F",
    "Model": "SIGMA DP2 Merrill",
    "Aperture": 5.6,
    "LensID": "Unknown (32776)"
  },
  "conversion_settings": {
    "compress": true,
    "denoise_intensity": 10,
    "highlight_recovery": false,
    "look_profile_path": "/profiles/look.dcp",
    "color_profile": "sRGB"
  }
}
```

Paths are absolute. `look_profile_path` is the resolved bundled/custom DCP path,
or `null` when disabled. `color_profile` uses the core names `sRGB`, `AdobeRGB`,
`ProPhotoRGB`, and `None`; it is not the selected look's name.

Metadata contains the available ExifTool Model, Aperture, and LensID values.
Fields may be absent, and the object may be empty if extraction fails. These are
not complete EXIF records or guessed camera values.

A Python command can read the request with:

```python
import json
import sys
from pathlib import Path

request = json.loads(sys.stdin.buffer.read().decode("utf-8"))
if request["schema_version"] != 1:
    raise ValueError("Unsupported hook schema")
output = Path(request["output_path"])
# Process this DNG and leave it at the same path. Never modify input_path.
```

## Completion and cancellation

Exit zero means success, provided the output still exists as a nonempty regular
file. This is an existence check, not full DNG validation. A nonzero exit, launch
error, or missing/empty output marks the file failed with a **post-processing**
message. The batch continues, and the published path remains available for reveal.

**Stop** cancels active hooks and the rest of the batch. On macOS/Linux the
command's process group receives SIGTERM, followed by SIGKILL after one second.
Windows uses `taskkill /T /F` to stop the foreground command tree. A file stopped
during post-processing is marked failed with a cancellation message, not returned
to the unconverted queue.

The app does not delete the published DNG on hook failure or cancellation. It
also does not undo a script's changes, recreate a deleted output, or remove the
script's work files. Prefer temporary output and atomic replacement within your
script. The app itself never modifies the source X3F; reconverting it produces a
fresh DNG. Arbitrary command side effects are not guaranteed to be idempotent.
