// DDD role: ValueObject
package schemas

// Collection value objects for the launch bounded context. Each one exists so
// that a schema field references a named collection instead of an inline list
// type, keeping the element contract in exactly one place.

// #CommandLine is the non-empty argv of a supervised child: argv[0] is the binary,
// every further element is an argument.
#CommandLine: [string, ...string]

// #EnvFilePathList is an ordered list of .env file paths, each relative to the
// profile directory and forbidden to escape it (ADR-0071).
#EnvFilePathList: [...string]

// #ServiceNameList is an ordered list of Service aliases.
#ServiceNameList: [...#ServiceName]

// #PatternList is an ordered list of regular expressions.
#PatternList: [...string]