// DDD role: ValueObject
package schemas

// Process-identity value objects for the subprocess bounded context. The pid
// floor of 2 excludes the init process and kernel threads from the allowable
// range, so a handle can never name a process substrate is unable to signal.

// #ProcessId is an OS process id; always >= 2.
#ProcessId: int & >=2

// #ProcessGroupId is a process-group leader id assigned by setsid() at spawn
// time (ADR-0053). killpg(process_group_id, signal) cascades to the whole
// subtree, which is why the id is modelled separately from #ProcessId.
#ProcessGroupId: int & >=2

// #ProcessIdentity is the OS identity pair of a spawned child: the process and
// the process group it leads. Embedded by #SubprocessHandle and #StackChild.
#ProcessIdentity: {
	// pid is the OS process ID of the spawned child. Always >= 2 to exclude
	// the init process and kernel threads from the allowable range.
	pid: #ProcessId

	// pgid is the process group ID assigned by setsid() at spawn time per ADR-0053.
	// killpg(pgid, signal) is used for cascade-kill so that child sub-processes
	// spawned by the child are also reaped.
	pgid: #ProcessGroupId
}