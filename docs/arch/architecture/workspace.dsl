// ADR-0040 introduces async job control-plane (substrate-jobs).
// ADR-0041 introduces optional filesystem index (substrate-fs-index).
// ADR-0042 capability-adapter factory pattern affects every adapter (no new container; cross-cutting).
// ADR-0043 SIMD runtime dispatch is cross-cutting; no new container.
// ADR-0044 no-subprocess policy is enforced via CI Rego (docs/arch/policies/no_subprocess.rego); no new container.
workspace "substrate" "MCP server exposing POSIX baseutils to LLM agents — secure, async-native, STDIO transport." {

    model {

        # External actors
        llmAgent = person "LLM Agent" "Human or automated agent issuing tool calls through an LLM."

        mcpHost = softwareSystem "MCP Host Client" "Orchestrator that spawns substrate and exchanges JSON-RPC messages over STDIO. Examples: Claude Desktop, VSCode Copilot." {
            tags "External"
        }

        localOs = softwareSystem "Local OS" "POSIX kernel, filesystem, process table, and archive utilities available on the host machine." {
            tags "External"
        }

        # substrate bounded system
        substrate = softwareSystem "substrate" "Rust MCP server that exposes POSIX baseutils as structured tools consumable by LLM agents." {

            mcpServer = container "substrate-mcp-server" "Composition root. Owns the rmcp runtime, STDIO transport, JSON-RPC dispatch loop, and tool registry." "Rust / rmcp" {
                tags "Server"
            }

            policy = container "substrate-policy" "Evaluates allowlists, path sandboxing, rate limits, and capability gates before any side-effecting operation." "Rust" {
                tags "Policy"
            }

            config = container "substrate-config" "Loads and validates runtime configuration and policy rules from environment and config files." "Rust / TOML" {
                tags "Config"
            }

            domain = container "substrate-domain" "Shared kernel: domain types, port traits, error taxonomy, and value objects. Zero infrastructure dependencies." "Rust" {
                tags "Domain"
            }

            fsQuery = container "substrate-fs-query" "Adapter for read-only filesystem operations: list, read, stat, search, glob." "Rust" {
                tags "Adapter"
            }

            fsMutation = container "substrate-fs-mutation" "Adapter for filesystem mutations: write, copy, move, delete, mkdir, chmod." "Rust" {
                tags "Adapter"
            }

            process = container "substrate-process" "Adapter for process management: spawn, exec, signal, stream stdout/stderr." "Rust" {
                tags "Adapter"
            }

            systemInfo = container "substrate-system-info" "Adapter for host introspection: CPU, memory, disk, network interfaces, OS metadata." "Rust" {
                tags "Adapter"
            }

            text = container "substrate-text" "Adapter for text processing: grep, sed-like replace, diff, encoding, line operations." "Rust" {
                tags "Adapter"
            }

            archive = container "substrate-archive" "Adapter for archive operations: tar, zip, gzip, zstd — pack, unpack, inspect." "Rust" {
                tags "Adapter"
            }

            jobs = container "substrate-jobs" "In-memory JobRegistry adapter for async control-plane. Tracks job state, CancellationToken handles, and progress notifications. Exposes job.status, job.result, job.cancel, job.list tool endpoints." "Rust" {
                tags "Adapter"
            }

            fsIndex = container "substrate-fs-index" "Optional filesystem index adapter (fs-index Cargo feature). Accelerates fs.find and fs.stat by maintaining a lightweight in-memory index updated at commit time." "Rust crate, opt-in" {
                tags "Adapter"
            }

            fsIndexMacosSys = container "substrate-fs-index-macos-sys" "Platform shim providing the macOS-specific FSEvents and kqueue backend bindings consumed by substrate-fs-index on Apple platforms." "Rust / macOS FFI" {
                tags "Platform"
            }

            signalSys = container "substrate-signal-sys" "Platform shim implementing SIGPIPE disposition (SIG_IGN at startup) and other signal-safety concerns required by ADR-0032. Linked only by substrate-mcp-server." "Rust / libc" {
                tags "Platform"
            }

            // ADR-0052: subprocess bounded context — optional Cargo feature 'subprocess' (default-OFF).
            // Hosts tokio::process::Command as the single permitted site per no_subprocess.rego amendment.
            subprocessAdapter = container "substrate-subprocess" "Adapter for child process spawning: validates binary allowlist, env filtering, cascading kill, stdout/stderr stream multiplex (ADR-0054), and orphan prevention (PR_SET_PDEATHSIG / watchdog pipe, ADR-0053)." "Rust crate, opt-in (feature subprocess)" {
                tags "Adapter" "OptionalFeature"
            }

            // ADR-0058: network-info bounded context — net.tcp_list, net.udp_list, net.tcp_stats, net.connection_count.
            networkInfo = container "substrate-network-info" "Adapter for network socket introspection: lists TCP/UDP sockets, aggregates per-connection stats, and resolves owner PIDs from kernel PCB tables (procfs on Linux, pcblist_n sysctl on macOS)." "Rust" {
                tags "Adapter"
            }

            // ADR-0063..0068: launch bounded context — declarative process orchestration OVER subprocess.
            // Optional Cargo feature 'launch'. Detached --supervise mode (ADR-0068) is the same binary.
            launch = container "substrate-launch" "Orchestration adapter for declarative multi-process stacks from .substrate.toml: TOFU trust gate (ADR-0064), depends_on DAG + reconciler reload (ADR-0065), distilled event stream (ADR-0066), lock-free mpsc/broadcast/watch fabric (ADR-0067), and the detached supervisor with zero-orphan governance (ADR-0068)." "Rust crate, opt-in (feature launch)" {
                tags "Adapter" "OptionalFeature"
            }
        }

        # External relationships
        llmAgent -> mcpHost "Issues prompts and consumes tool results"
        mcpHost -> mcpServer "Spawns and communicates via JSON-RPC over STDIO"

        # Internal relationships — mcpServer core
        mcpServer -> domain "Uses ports from"
        mcpServer -> config "Loads policy and runtime config from"
        mcpServer -> fsQuery "Routes filesystem-query tool calls to"
        mcpServer -> fsMutation "Routes filesystem-mutation tool calls to"
        mcpServer -> process "Routes process tool calls to"
        mcpServer -> systemInfo "Routes system-info tool calls to"
        mcpServer -> text "Routes text-processing tool calls to"
        mcpServer -> archive "Routes archive tool calls to"

        # Adapter -> domain (port implementations)
        fsQuery -> domain "Implements port from"
        fsMutation -> domain "Implements port from"
        process -> domain "Implements port from"
        systemInfo -> domain "Implements port from"
        text -> domain "Implements port from"
        archive -> domain "Implements port from"

        # Policy validation (side-effecting adapters only)
        fsQuery -> policy "Validates path access via"
        fsMutation -> policy "Validates mutation via"
        process -> policy "Validates spawn permissions via"
        archive -> policy "Validates archive paths via"

        # Adapter -> OS (runtime syscalls)
        fsQuery -> localOs "Reads from"
        fsMutation -> localOs "Mutates"
        process -> localOs "Spawns and signals processes on"
        systemInfo -> localOs "Inspects host metrics from"
        text -> localOs "Reads file content from"
        archive -> localOs "Streams archive data from/to"

        # Job control-plane relationships (ADR-0040)
        mcpServer -> jobs "Submits jobs, polls status, cancels via CancellationToken"
        jobs -> domain "Implements JobRegistryPort from"
        mcpHost -> mcpServer "Calls job.status, job.result, job.cancel, job.list; receives progress notifications"

        # Filesystem index relationships (ADR-0041)
        fsQuery -> fsIndex "Consults index via FsIndexPort (when fs-index feature enabled)"
        fsIndex -> domain "Implements FsIndexPort from"
        fsMutation -> fsIndex "Write-through update at commit time"

        # Platform shim relationships
        fsIndex -> fsIndexMacosSys "Uses macOS FSEvents/kqueue backend on Apple platforms"
        mcpServer -> signalSys "Calls signal-safety setup at startup"

        # Subprocess adapter relationships (ADR-0052, optional feature 'subprocess')
        mcpServer -> subprocessAdapter "Uses (when subprocess feature enabled)"
        subprocessAdapter -> jobs "Registers SubprocessHandle as JobEntry"
        subprocessAdapter -> policy "Validates binary path and env allowlist"
        subprocessAdapter -> domain "Implements SubprocessPort"
        subprocessAdapter -> localOs "Spawns and signals child process groups on"
        subprocessAdapter -> mcpServer "Forwards stdout/stderr stream chunks as notifications/progress (ADR-0054 dispatcher task)"

        # Network-info adapter relationships (ADR-0058)
        mcpServer -> networkInfo "Routes network-info tool calls to"
        networkInfo -> domain "Implements NetworkInfoPort from"
        networkInfo -> localOs "Reads TCP/UDP socket tables from"

        # Launch adapter relationships (ADR-0063..0068, optional feature 'launch')
        mcpServer -> launch "Routes launch.* tool calls to (when launch feature enabled)"
        launch -> domain "Implements LaunchPort; consumes SubprocessPort (concrete substrate-subprocess adapter injected by the mcp-server root); each Service materializes to one subprocess.spawn"
        launch -> policy "Validates Profile trust and per-Service spawn via"
        launch -> jobs "Registers Stack bring-up as a Task/JobEntry (ADR-0049)"
        launch -> localOs "Spawns the detached --supervise supervisor; owns control FIFO and durable state-file"
        launch -> mcpServer "Forwards distilled lifecycle/semantic events as resource-updated notifications (ADR-0066)"
    }

    views {

        systemContext substrate "context" "System context: substrate and its external dependencies." {
            include *
            autolayout lr
        }

        container substrate "containers" "Container-level decomposition of the substrate MCP server." {
            include *
            autolayout tb
        }

        styles {
            element "Person" {
                shape Person
                background "#1168bd"
                color "#ffffff"
            }
            element "External" {
                background "#999999"
                color "#ffffff"
            }
            element "Server" {
                background "#2d6a4f"
                color "#ffffff"
            }
            element "Policy" {
                background "#b5451b"
                color "#ffffff"
            }
            element "Config" {
                background "#6b4f8e"
                color "#ffffff"
            }
            element "Domain" {
                background "#1a5276"
                color "#ffffff"
            }
            element "Adapter" {
                background "#1e8449"
                color "#ffffff"
            }
            element "Platform" {
                background "#7d6608"
                color "#ffffff"
            }
            element "Software System" {
                background "#444444"
                color "#ffffff"
            }
            element "Container" {
                background "#438dd5"
                color "#ffffff"
            }
        }

        theme default
    }
}
