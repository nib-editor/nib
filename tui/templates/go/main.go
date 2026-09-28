// {{name}} shows how many words the shown buffer has in the status line,
// and answers the {{name}}.count command with the number. Replace it with
// what your plugin does.
package main

import (
	"strconv"
	"strings"

	nib "github.com/nib-editor/nib/sdk/go"
	"github.com/nib-editor/nib/sdk/go/nib/plugin/commands"
	"github.com/nib-editor/nib/sdk/go/nib/plugin/events"
	"github.com/nib-editor/nib/sdk/go/nib/plugin/types"
	"github.com/nib-editor/nib/sdk/go/nib/plugin/ui"
	"github.com/nib-editor/nib/sdk/go/nib/plugin/view"
	"go.bytecodealliance.org/cm"
)

type plugin struct {
	nib.Base
}

func (plugin) Init(string) error {
	commands.Register("count", "The number of words in the shown buffer")
	show()
	return nil
}

func (plugin) RunCommand(name, args string) (string, error) {
	if name == "count" {
		return strconv.Itoa(count()), nil
	}
	return nib.Base{}.RunCommand(name, args)
}

func (plugin) OnEvent(ev events.Event) {
	// The events carry handles to buffers, which are ours to drop.
	if buffer := ev.BufferOpened(); buffer != nil {
		buffer.ResourceDrop()
	}
	if change := ev.BufferChanged(); change != nil {
		change.Buffer.ResourceDrop()
	}
	show()
}

// count counts the words of the buffer in the focused view.
func count() int {
	shown := view.Active()
	defer shown.ResourceDrop()
	buffer := shown.Buffer()
	defer buffer.ResourceDrop()
	text := buffer.Slice(0, buffer.Len())
	if text.IsErr() {
		return 0
	}
	return len(strings.Fields(*text.OK()))
}

func show() {
	words := count()
	label := strconv.Itoa(words) + " words"
	if words == 1 {
		label = "1 word"
	}
	line := []types.Span{{Text: label, Style: ""}}
	ui.SetStatus("words", ui.SideRight, 20, ui.StyledLine(cm.ToList(line)))
}

func init() {
	nib.Register(plugin{})
}

func main() {}
