// Interop helper for grob's tests.
//
//	go run . gen <dir>        writes Go-encoded fixtures read by tests/interop.rs
//	go run . verify <file>    decodes the Rust-encoded stream built in
//	                          tests/interop.rs and prints each value as JSON
//	go run . verify-orders <file>
//	                          decodes Rust-serialized Orders (tests/serde.rs)
//	                          and compares them with sampleOrders
package main

import (
	"bytes"
	"encoding/gob"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"reflect"
	"strconv"
	"strings"
)

// UUID mimics github.com/google/uuid.UUID: a BinaryMarshaler.
type UUID [16]byte

func (u UUID) MarshalBinary() ([]byte, error) { return u[:], nil }

func (u *UUID) UnmarshalBinary(data []byte) error {
	if len(data) != 16 {
		return fmt.Errorf("invalid UUID length %d", len(data))
	}
	copy(u[:], data)
	return nil
}

// Color is a TextMarshaler, which encoding/gob ignores (its support is
// commented out in type.go): it travels as a plain struct.
type Color struct{ R, G, B uint8 }

func (c Color) MarshalText() ([]byte, error) {
	return []byte(fmt.Sprintf("#%02x%02x%02x", c.R, c.G, c.B)), nil
}

func (c *Color) UnmarshalText(text []byte) error {
	_, err := fmt.Sscanf(string(text), "#%02x%02x%02x", &c.R, &c.G, &c.B)
	return err
}

// Version is a GobEncoder.
type Version struct{ Major, Minor int }

func (v Version) GobEncode() ([]byte, error) {
	return []byte(fmt.Sprintf("%d.%d", v.Major, v.Minor)), nil
}

func (v *Version) GobDecode(data []byte) error {
	major, minor, ok := strings.Cut(string(data), ".")
	if !ok {
		return fmt.Errorf("invalid version %q", data)
	}
	var err error
	if v.Major, err = strconv.Atoi(major); err != nil {
		return err
	}
	v.Minor, err = strconv.Atoi(minor)
	return err
}

type Primitives struct {
	Bool   bool
	Int    int64
	Neg    int32
	Uint   uint64
	Float  float64
	String string
	Bytes  []byte
	Zero   int // omitted on the wire
}

type Inner struct {
	Name string
}

type Nested struct {
	ID     UUID
	Inner  Inner
	Tags   []string
	Counts map[string]int
	Fixed  [3]int8
	Ptr    *string
}

type Point struct{ X, Y int }

type Holder struct {
	A int
	I any
	J any
	B string
}

type Deep struct{ Inner any }

type Node struct {
	Value int
	Next  *Node
}

type External struct {
	ID      UUID
	Color   Color
	Version Version
	Zero    Inner             // zero structs are still sent
	Empty   map[string]string // empty but non-nil maps are sent
	Nil     *Inner            // nil pointers are not
}

// Everything is what the Rust encoder produces in tests/interop.rs.
type Everything struct {
	Int        int64
	Uint       uint16
	Float      float32
	Bool       bool
	String     string
	Bytes      []byte
	Array      [2]Point
	Slice      []Point
	Map        map[string][]int
	Nested     Point
	Iface      any
	IfaceInt   any
	IfaceSlice any
	IfaceNil   any
	List       *Node
	ID         UUID
	Color      Color
	Version    Version
}

// Order mirrors the serde structs in tests/serde.rs.
type Item struct {
	SKU   string
	Count uint16
	Price float32
}

type Order struct {
	ID         UUID
	Customer   string
	Items      []Item
	Tags       []string
	Quantities map[string]uint32
	Total      float64
	Paid       bool
	Note       *string
	Shipping   *Point
	Position   [2]int
	Payload    []byte
	Extra      any
	Parent     *Order
	Version    Version
	Status     string
}

func sampleOrders() []Order {
	note := "leave at door"
	return []Order{
		{
			ID:         UUID{1, 15: 9},
			Customer:   "ann",
			Items:      []Item{{"a-1", 2, 9.5}, {SKU: "b-2"}},
			Tags:       []string{"gift", ""},
			Quantities: map[string]uint32{"a-1": 2, "b-2": 0},
			Total:      19,
			Paid:       true,
			Note:       &note,
			Shipping:   &Point{3, 4},
			Position:   [2]int{-1, 0},
			Payload:    []byte{0, 1},
			Extra:      Point{5, 6},
			Parent:     &Order{Customer: "ann's mom", Items: []Item{{Count: 1}}, Version: Version{1, 0}},
			Version:    Version{2, 3},
			Status:     "shipped",
		},
		{Customer: "bob", Extra: 42},
	}
}

func verifyOrders(file string) {
	data, err := os.ReadFile(file)
	if err != nil {
		panic(err)
	}
	dec := gob.NewDecoder(bytes.NewReader(data))
	var orders []Order
	for {
		var order Order
		err := dec.Decode(&order)
		if errors.Is(err, io.EOF) {
			break
		}
		if err != nil {
			fmt.Fprintln(os.Stderr, err)
			os.Exit(1)
		}
		orders = append(orders, order)
	}
	if want := sampleOrders(); !reflect.DeepEqual(orders, want) {
		got, _ := json.Marshal(orders)
		expected, _ := json.Marshal(want)
		fmt.Fprintf(os.Stderr, "got  %s\nwant %s\n", got, expected)
		os.Exit(1)
	}
	fmt.Println("ok")
}

func write(dir, name string, values ...any) {
	var buf bytes.Buffer
	enc := gob.NewEncoder(&buf)
	for _, value := range values {
		if err := enc.Encode(value); err != nil {
			panic(err)
		}
	}
	if err := os.WriteFile(filepath.Join(dir, name), buf.Bytes(), 0o644); err != nil {
		panic(err)
	}
}

func list(n int) *Node {
	var head *Node
	for i := n; i > 0; i-- {
		head = &Node{Value: i, Next: head}
	}
	return head
}

func gen(dir string) {
	write(dir, "primitives.gob", Primitives{
		Bool: true, Int: 1 << 40, Neg: -129, Uint: 255, Float: -17.5,
		String: "héllo", Bytes: []byte{0, 1, 2, 0xff},
	})
	ptr := "pointed"
	write(dir, "nested.gob", Nested{
		ID:     UUID{0x12, 0x34, 15: 0xff},
		Inner:  Inner{Name: "inner"},
		Tags:   []string{"a", "b"},
		Counts: map[string]int{"x": 1},
		Fixed:  [3]int8{-1, 0, 1},
		Ptr:    &ptr,
	})
	write(dir, "singleton.gob", []int{1, -2, 3})
	// One encoder: types are sent once, interface types inline, mid-value.
	write(dir, "interfaces.gob",
		Holder{A: 1, I: Point{3, 4}, J: 7, B: "z"},
		Deep{Inner: Deep{Inner: Point{1, 2}}},
		Holder{I: []any{Point{5, 6}, "s", nil, []int{1, 2}}, J: complex(1, 2)},
		Holder{A: 2},
	)
	write(dir, "stream.gob",
		Point{1, 2},
		Point{},
		complex(1.5, -2),
		"str",
		map[string]Point{"a": {1, 2}},
		[2]bool{true, false},
		uint8(200),
		float32(0.5),
		[]Point{},
	)
	write(dir, "list.gob", list(300))
	write(dir, "external.gob", External{
		ID:      UUID{1, 15: 2},
		Color:   Color{0xff, 0x80, 0x00},
		Version: Version{1, 2},
		Empty:   map[string]string{},
	})
	orders := sampleOrders()
	write(dir, "orders.gob", orders[0], orders[1])
}

func verify(file string) {
	data, err := os.ReadFile(file)
	if err != nil {
		panic(err)
	}
	dec := gob.NewDecoder(bytes.NewReader(data))
	var first, second Everything
	var c complex128
	var n int
	for _, target := range []any{&first, &c, &n, &second} {
		if err := dec.Decode(target); err != nil {
			fmt.Fprintln(os.Stderr, err)
			os.Exit(1)
		}
	}
	if err := dec.Decode(new(int)); err != io.EOF {
		fmt.Fprintln(os.Stderr, "expected EOF, got", err)
		os.Exit(1)
	}
	for _, value := range []any{first, fmt.Sprint(c), n, second} {
		out, err := json.Marshal(value)
		if err != nil {
			panic(err)
		}
		fmt.Println(string(out))
	}
}

func main() {
	gob.Register(Point{})
	gob.Register(Deep{})
	gob.Register([]any{})
	switch os.Args[1] {
	case "gen":
		gen(os.Args[2])
	case "verify":
		verify(os.Args[2])
	case "verify-orders":
		verifyOrders(os.Args[2])
	}
}
