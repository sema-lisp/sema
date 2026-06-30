---
name: "proc/spawn"
module: "process"
section: "Processes"
---

Spawn a subprocess and return an integer handle. `(proc/spawn ["cargo" "test"])` or `(proc/spawn argv {:cwd "path" :env {"KEY" "val"}})`. Unlike `shell`, stdout/stderr stream live into buffers you poll with `proc/read-stdout`/`proc/read-stderr`.
