// Package nib is the Go SDK for nib plugins.
//
// Implement [Plugin], embedding [Base] for what you leave out, pass it to
// [Register] in an init function, and build with TinyGo for wasip2:
//
//	tinygo build -target=wasip2 \
//	  --wit-package "$(go list -m -f '{{.Dir}}' github.com/nib-editor/nib/sdk/go)/wit" \
//	  --wit-world plugin -o plugin.wasm .
//
// The editor's API is in the generated packages under nib/plugin/, one per
// WIT interface, such as nib/plugin/buffer.
package nib

import (
	"errors"

	"github.com/nib-editor/nib/sdk/go/nib/plugin/events"
	"github.com/nib-editor/nib/sdk/go/nib/plugin/guest"
	"github.com/nib-editor/nib/sdk/go/nib/plugin/types"
	"go.bytecodealliance.org/cm"
)

// Plugin is what the editor calls into.
type Plugin interface {
	// Init is called once after loading, with the plugin's [settings] from
	// plugins/<name>.toml as JSON.
	Init(config string) error
	// HandleKey gets a key that reached one of the plugin's input layers,
	// and reports whether it used it; otherwise the layer below gets it.
	HandleKey(ev types.KeyEvent) bool
	// HandlePaste gets text pasted into the terminal that reached one of
	// the plugin's input layers, as one piece, and reports whether it used
	// it.
	HandlePaste(text string) bool
	// RunCommand runs one of the plugin's registered commands, by the name
	// it was registered with. Arguments and the result are JSON.
	RunCommand(name, args string) (string, error)
	// OnEvent gets the events the plugin listens to.
	OnEvent(ev events.Event)
}

// Base does nothing, for plugins to embed and leave out what they do not
// need.
type Base struct{}

func (Base) Init(string) error { return nil }

func (Base) HandleKey(types.KeyEvent) bool { return false }

func (Base) HandlePaste(string) bool { return false }

func (Base) RunCommand(name, _ string) (string, error) {
	return "", errors.New("no command " + name)
}

func (Base) OnEvent(events.Event) {}

// Register makes p the plugin the editor calls.
func Register(p Plugin) {
	guest.Exports.Init = func(config string) cm.Result[string, struct{}, string] {
		if err := p.Init(config); err != nil {
			return cm.Err[cm.Result[string, struct{}, string]](err.Error())
		}
		return cm.OK[cm.Result[string, struct{}, string]](struct{}{})
	}
	guest.Exports.HandleKey = func(ev types.KeyEvent) guest.KeyResult {
		if p.HandleKey(ev) {
			return guest.KeyResultHandled
		}
		return guest.KeyResultPass
	}
	guest.Exports.HandlePaste = func(text string) guest.KeyResult {
		if p.HandlePaste(text) {
			return guest.KeyResultHandled
		}
		return guest.KeyResultPass
	}
	guest.Exports.RunCommand = func(name, args string) cm.Result[string, string, string] {
		result, err := p.RunCommand(name, args)
		if err != nil {
			return cm.Err[cm.Result[string, string, string]](err.Error())
		}
		return cm.OK[cm.Result[string, string, string]](result)
	}
	guest.Exports.OnEvent = p.OnEvent
}
