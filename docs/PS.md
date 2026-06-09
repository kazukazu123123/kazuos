# ps

Prints a snapshot of all running processes.

## Output format

```
  PID  STATE    %CPU     MEM     NAME
    0  R          3.2%    2866KiB  kernel
    1  S          0.1%      23KiB  /bin/shell.kxe
    2  R          0.4%      19KiB  /bin/ps.kxe
```

| Column | Description |
| --- | --- |
| `PID` | Process ID. `0` is the kernel pseudo-entry. |
| `STATE` | Process state (see below). |
| `%CPU` | Share of total CPU time (TSC ticks) since boot. |
| `MEM` | Allocated memory. For PID 0 this is PMM used minus all user process memory. |
| `NAME` | Executable path, or `kernel` for PID 0. |

## STATE values

| Code | Meaning |
| --- | --- |
| `Ready` | Ready — the process is runnable and waiting to be scheduled (`ProcessState::Ready`). |
| `Running` | Running — the process is currently executing on the CPU (`ProcessState::Running`). |
| `Sleep` | Sleeping — the process is blocked waiting for an event such as keyboard input, a child process, or a timer (`ProcessState::Sleeping`). |
| `?` | Unknown state. |

The kernel row (`PID 0`) always shows `Running` because idle/kernel ticks are not modeled as a blockable process.

## CPU accounting

`%CPU` is computed as:

```
process_tsc_ticks / (kernel_ticks + idle_ticks + user_ticks) * 100
```

- **kernel** ticks: timer fired while no user process was active and the CPU was not in the idle hlt loop.
- **idle** ticks: timer fired while the scheduler hlt loop was running (no ready process).
- **user** ticks: timer fired while a user-mode process was on-CPU.

The kernel row shows kernel ticks only; idle time is excluded so it does not inflate the displayed value.
