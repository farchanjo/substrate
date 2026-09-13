// DDD role: ValueObject
package schemas

// Trust surface of the launch bounded context: the user-scope trust store entry
// and the operator policy that governs inline blessing. Per ADR-0064.

// #TrustRecord is one bless entry in the user-scope trust store (trust.toml). It binds a
// canonical Profile path to its full inode-and-content identity tuple, re-verified on
// every load to defeat permission-flip and rewrite attacks. Per ADR-0064.
#TrustRecord: {
	// path is the absolute canonical path of the trusted .substrate.toml.
	path: #AbsolutePath

	// dev / ino / uid / mode are the inode identity captured by fstat at bless time and
	// re-checked on every load. mode is masked to the permission bits (0o7777 = 4095).
	dev:  int & >=0
	ino:  int & >=0
	uid:  int & >=0
	mode: int & >=0 & <=4095

	// content is the prefixed content hash of the file at bless time (blake3 or sha256).
	content: string & =~"^(blake3|sha256):"

	// blessed_at is the RFC 3339 timestamp the record was created.
	blessed_at: #Timestamp
}

// #LaunchOperatorConfig is the user-scope launch operator policy, loaded at startup
// from ${XDG_CONFIG_HOME:-~/.config}/substrate/launch.toml (mode 0600, owner-checked).
// It lives OUTSIDE any repository so a cloned Profile cannot authorize its own
// blessing (trust-order confusion). Per ADR-0064.
#LaunchOperatorConfig: {
	// auto_bless_paths lists absolute canonical path prefixes for which launch.up may
	// bless a new content/identity tuple inline instead of requiring launch.trust.
	// Empty (default) means every new Profile needs an explicit launch.trust ceremony.
	// A repository cannot add itself here; only the operator edits user-scope config.
	auto_bless_paths: #AbsolutePathList | *[]
}