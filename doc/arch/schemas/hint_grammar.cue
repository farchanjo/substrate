// DDD role: ValueObject
package schemas

// #HintKey enumerates the five stable structured-hint keys
// that substrate tools may embed in their response payloads.
#HintKey:
	"next_action_suggested" |
	"alternative_tool" |
	"confirm_destructive" |
	"quota_status" |
	"error_recovery"

// #HintValueMaxTokens is the per-hint value token budget.
// Hint values MUST NOT exceed this limit to keep response payloads compact.
#HintValueMaxTokens: 25

// #Hint is a single structured hint attached to a tool result.
#Hint: {
	// key identifies the semantic category of the hint.
	key: #HintKey

	// value is the hint payload; must stay within #HintValueMaxTokens.
	value: #ShortText
}

// #HintsMap is the canonical map of hint keys to their string values
// as embedded in a #ToolResult. All keys are optional; absent keys
// signal that the hint is not applicable for this invocation.
// Values are carried as text here; #Hints in tool_card.cue is the typed
// counterpart used on the wire.
#HintsMap: {
	next_action_suggested?: #ShortText
	alternative_tool?:      #ToolName
	confirm_destructive?:   #ShortText
	quota_status?:          #ShortText
	error_recovery?:        #ShortText
}