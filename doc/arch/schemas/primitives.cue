// DDD role: ValueObject
package schemas

// Constrained primitives shared across bounded-context schemas.
//
// Each definition exists so a field can carry a named, constrained type instead
// of a bare primitive: the constraint is the whole content and the name is the
// documentation. A field that has a more specific home (a tier enum, a domain
// identifier) references that type instead of one of these.

// #Timestamp is an RFC 3339 UTC instant with a mandatory Z suffix.
#Timestamp: string & =~"^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}(\\.[0-9]+)?Z$"

// #ToolName is a fully-qualified tool identifier, `namespace_name`.
#ToolName: string & =~"^(fs|proc|sys|text|archive|job|subprocess|launch)_[a-z][a-z0-9_]*$"

// #AbsolutePath is a path that begins at the filesystem root.
#AbsolutePath: string & =~"^/"

// #Base64Blob is a non-empty base64 payload.
#Base64Blob: string & != ""

// #ShortText is a non-empty human-readable string.
#ShortText: string & != ""

// #Flag is a boolean predicate.
#Flag: bool

// #Counter is a non-negative integral count.
#Counter: uint

// #ExitCode is a process exit status; absent, not negative, when a signal
// ended the child.
#ExitCode: int & >=0

// #Score is a non-negative ranking weight.
#Score: number & >=0.0