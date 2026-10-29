// Command api-client-go is a client of Muster's daemon API, written against bindings generated
// from `muster docs api --schema` and nothing else of Muster's. It subscribes to every topic,
// makes a pane, and reads the pane's text, printing what the daemon said at each step.
//
// Usage: api-client-go [socket]. Without one it uses $MUSTER_DAEMON_SOCKET, as a pane has.
package main

import (
	"encoding/binary"
	"fmt"
	"io"
	"net"
	"os"
	"strings"
	"time"

	"google.golang.org/protobuf/proto"

	api "github.com/amterp/muster/extras/api-client-go/musterapi"
)

// The version of the schema this client was generated from, which the daemon judges it by.
var speaks = &api.Version{Major: 0, Minor: 2}

func main() {
	socket := os.Getenv("MUSTER_DAEMON_SOCKET")
	if len(os.Args) > 1 {
		socket = os.Args[1]
	}
	if socket == "" {
		fail("name a daemon's socket, or run this in a Muster pane")
	}
	connection, err := net.Dial("unix", socket)
	if err != nil {
		fail("could not connect to %s: %v", socket, err)
	}
	defer connection.Close()
	client := &client{connection: connection}

	client.write(&api.Hello{
		Kind:   api.Kind_KIND_API,
		Api:    speaks,
		Client: &api.Client{Name: "api-client-go", Version: "1"},
	})
	answer := &api.HelloAnswer{}
	client.read(answer)
	welcome := answer.GetWelcome()
	if welcome == nil {
		fail("refused: %s", answer.GetRefused().GetReason())
	}
	features := welcome.GetMachine().GetFeatures()
	fmt.Printf("welcomed by a daemon speaking API %d.%d\n", welcome.GetApi().GetMajor(), welcome.GetApi().GetMinor())
	fmt.Printf("  it serves %s\n", strings.Join(features.GetRequests(), ", "))
	fmt.Printf("  and the topics %v\n", features.GetTopics())

	subscribed := client.ask(&api.Request{Request: &api.Request_Api{Api: &api.ApiRequest{
		Request: &api.ApiRequest_Subscribe{Subscribe: &api.Subscribe{Topics: []api.Topic{
			api.Topic_TOPIC_PANES, api.Topic_TOPIC_MACHINES, api.Topic_TOPIC_MESSAGES, api.Topic_TOPIC_ATTENTION,
		}}},
	}}})
	snapshot := subscribed.GetSnapshot()
	fmt.Printf("subscribed: %v, epoch %d, current to %d\n", subscribed.GetOutcome(), snapshot.GetEpoch(), snapshot.GetThrough())
	fmt.Printf("  %d pane(s), %d machine(s), %d group(s), %d pane(s) asking for the human\n",
		len(snapshot.GetPanes()), len(snapshot.GetMachines()), len(snapshot.GetGroups()),
		len(snapshot.GetAttention().GetAsking()))

	directory, err := os.Getwd()
	if err != nil {
		fail("no working directory to make the pane in: %v", err)
	}
	made := client.ask(&api.Request{Request: &api.Request_Pane{Pane: &api.PaneRequest{
		Request: &api.PaneRequest_New{New: &api.NewPane{
			Operation: fmt.Sprintf("api-client-go-%d", time.Now().UnixNano()),
			Run:       "printf 'hello from a pane made in Go\\n'; exec sleep 600",
			Directory: directory,
			Name:      "from-go",
		}},
	}}})
	pane := made.GetMade().GetPane()
	fmt.Printf("pane.new: %v, pane %s (%s), through %d\n", made.GetOutcome(), pane, made.GetMade().GetCallsign(), made.GetThrough())

	for attempt := 0; ; attempt++ {
		read := client.ask(&api.Request{Request: &api.Request_Api{Api: &api.ApiRequest{
			Request: &api.ApiRequest_ReadPane{ReadPane: &api.ReadPane{
				Pane: pane, Rows: &api.ReadPane_Newest{Newest: 5},
			}},
		}}})
		text := read.GetPaneText().GetText()
		if strings.Contains(text, "hello") || attempt == 50 {
			fmt.Printf("api.read_pane: %v, rows %d from row %d:\n", read.GetOutcome(), read.GetPaneText().GetRows(), read.GetPaneText().GetFirstRow())
			fmt.Printf("  %q\n", text)
			return
		}
		time.Sleep(100 * time.Millisecond)
	}
}

// One connection to the daemon, and the correlation ids it has used.
type client struct {
	connection net.Conn
	asked      int
}

// Sends `request` under a fresh correlation id and returns its answer, printing every event that
// came before it.
func (c *client) ask(request *api.Request) *api.Answer {
	c.asked++
	request.Correlation = fmt.Sprint(c.asked)
	c.write(request)
	for {
		message := &api.DaemonMessage{}
		c.read(message)
		switch arm := message.GetMessage().(type) {
		case *api.DaemonMessage_Answer:
			if arm.Answer.GetCorrelation() == request.Correlation {
				return arm.Answer
			}
		case *api.DaemonMessage_Event:
			event := arm.Event
			cause := "caused by nobody's request on this connection"
			if event.GetCorrelation() != "" {
				cause = fmt.Sprintf("caused by request %s", event.GetCorrelation())
			}
			fmt.Printf("  event %d: %s, %s\n", event.GetNumber(), kind(event), cause)
		case *api.DaemonMessage_Ended:
			fail("the daemon ended the connection: %v %s", arm.Ended.GetReason(), arm.Ended.GetDetail())
		}
	}
}

func kind(event *api.Event) string {
	switch arm := event.GetEvent().(type) {
	case *api.Event_PaneAppeared:
		return "pane " + arm.PaneAppeared.GetPane().GetPane() + " appeared"
	case *api.Event_PaneChanged:
		pane := arm.PaneChanged.GetPane()
		return fmt.Sprintf("pane %s changed, %v", pane.GetPane(), pane.GetState())
	case *api.Event_PaneClosed:
		return "pane " + arm.PaneClosed.GetPane() + " closed"
	default:
		return fmt.Sprintf("%T", arm)
	}
}

// A frame is a four-byte big-endian length, then one message.
func (c *client) write(message proto.Message) {
	body, err := proto.Marshal(message)
	if err != nil {
		fail("could not encode a %T: %v", message, err)
	}
	frame := binary.BigEndian.AppendUint32(nil, uint32(len(body)))
	if _, err := c.connection.Write(append(frame, body...)); err != nil {
		fail("could not write to the daemon: %v", err)
	}
}

func (c *client) read(message proto.Message) {
	var length [4]byte
	if _, err := io.ReadFull(c.connection, length[:]); err != nil {
		fail("the daemon hung up: %v", err)
	}
	body := make([]byte, binary.BigEndian.Uint32(length[:]))
	if _, err := io.ReadFull(c.connection, body); err != nil {
		fail("the daemon hung up mid-frame: %v", err)
	}
	if err := proto.Unmarshal(body, message); err != nil {
		fail("the daemon sent something that is not a %T: %v", message, err)
	}
}

func fail(format string, values ...any) {
	fmt.Fprintf(os.Stderr, "api-client-go: "+format+"\n", values...)
	os.Exit(1)
}
