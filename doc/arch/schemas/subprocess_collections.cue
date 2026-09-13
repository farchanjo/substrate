// DDD role: ValueObject
package schemas

// Collection value objects for the subprocess bounded context. Each one exists so
// that a schema field references a named collection instead of an inline list
// type, keeping the element contract in exactly one place.

// #ArgumentList is the argv tail of a child process: every argument after argv[0].
#ArgumentList: [...string]

// #EnvNameList names the environment variables (not their values) that may be
// inherited from the substrate process environment by a child process.
#EnvNameList: [...string]

// #EnvOverrideMap maps environment variable names to explicit values set in the
// child environment, overriding anything inherited through #EnvNameList.
#EnvOverrideMap: [string]: string

// #AbsolutePathList is an ordered list of absolute filesystem paths.
#AbsolutePathList: [...#AbsolutePath]

// #LineList is an ordered list of decoded UTF-8 output lines, newlines excluded.
#LineList: [...string]