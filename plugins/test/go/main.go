// Test plugin written in Go with the Go SDK: it types keys into the buffer,
// answers the echo command, and shows a status item when test-events pings.
package main

import (
	nib "github.com/nib-editor/nib/sdk/go"
	"github.com/nib-editor/nib/sdk/go/nib/plugin/commands"
	"github.com/nib-editor/nib/sdk/go/nib/plugin/view"
	"github.com/nib-editor/nib/sdk/go/nib/plugin/events"
	"github.com/nib-editor/nib/sdk/go/nib/plugin/input"
	"github.com/nib-editor/nib/sdk/go/nib/plugin/types"
	"github.com/nib-editor/nib/sdk/go/nib/plugin/ui"
	"go.bytecodealliance.org/cm"
)

type plugin struct {
	nib.Base
}

func (plugin) Init(string) error {
	input.PushLayer()
	commands.Register("echo", "test: returns its arguments")
	return nil
}

func (plugin) HandleKey(ev types.KeyEvent) bool {
	c := ev.Code.Char()
	if c == nil || ev.Modifiers != 0 {
		return false
	}
	view := view.Active()
	var edits []types.Edit
	for _, r := range view.Selection().Ranges.Slice() {
		edits = append(edits, types.Edit{Start: r.Head, End: r.Head, Text: string(*c)})
	}
	result := view.Apply(view.Buffer().Version(), cm.ToList(edits), cm.None[types.Selection](), types.UndoModeMerge)
	return !result.IsErr()
}

func (plugin) RunCommand(name, args string) (string, error) {
	if name == "echo" {
		return args, nil
	}
	return nib.Base{}.RunCommand(name, args)
}

func (plugin) OnEvent(ev events.Event) {
	if custom := ev.Custom(); custom != nil && custom.Name == "test-events.ping" {
		span := types.Span{Text: "pinged", Style: ""}
		ui.SetStatus("go", ui.SideRight, 0, ui.StyledLine(cm.ToList([]types.Span{span})))
	}
}

func init() {
	nib.Register(plugin{})
}

func main() {}
