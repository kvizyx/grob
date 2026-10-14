package main

// Go's encoding/gob on the data of grob's benches/gob.rs, for comparison:
//
//	go test -bench . -benchmem

import (
	"bytes"
	"encoding/gob"
	"fmt"
	"testing"
)

// makeOrders builds the same orders as orders() in benches/gob.rs.
func makeOrders(count int) []Order {
	orders := make([]Order, count)
	for i := range orders {
		var items []Item
		quantities := map[string]uint32{}
		total := 0.0
		for j := 0; j < i%5+1; j++ {
			item := Item{SKU: fmt.Sprintf("sku-%d", j), Count: uint16(j), Price: float32(j) * 1.5}
			items = append(items, item)
			quantities[item.SKU] = uint32(item.Count)
			total += float64(item.Price) * float64(item.Count)
		}
		var id UUID
		for b := range id {
			id[b] = byte(i + b)
		}
		payload := make([]byte, 32)
		for b := range payload {
			payload[b] = byte(i * b)
		}
		var note *string
		if i%3 == 0 {
			n := fmt.Sprintf("note %d", i)
			note = &n
		}
		var extra any = i
		if i%2 == 0 {
			extra = Point{i, 1}
		}
		orders[i] = Order{
			ID:         id,
			Customer:   fmt.Sprintf("customer-%d", i),
			Items:      items,
			Tags:       []string{fmt.Sprintf("t%d", i%3), "x"},
			Quantities: quantities,
			Total:      total,
			Paid:       i%2 == 0,
			Note:       note,
			Position:   [2]int{i, -i},
			Payload:    payload,
			Extra:      extra,
			Version:    Version{1, i % 10},
		}
	}
	return orders
}

func init() {
	gob.Register(Point{})
}

func encodeOrders(b *testing.B, orders []Order) []byte {
	var buf bytes.Buffer
	if err := gob.NewEncoder(&buf).Encode(orders); err != nil {
		b.Fatal(err)
	}
	return buf.Bytes()
}

func BenchmarkOrdersEncode(b *testing.B) {
	orders := makeOrders(1000)
	b.SetBytes(int64(len(encodeOrders(b, orders))))
	for b.Loop() {
		encodeOrders(b, orders)
	}
}

func BenchmarkOrdersDecode(b *testing.B) {
	data := encodeOrders(b, makeOrders(1000))
	b.SetBytes(int64(len(data)))
	for b.Loop() {
		var orders []Order
		if err := gob.NewDecoder(bytes.NewReader(data)).Decode(&orders); err != nil {
			b.Fatal(err)
		}
	}
}

func BenchmarkStreamEncode(b *testing.B) {
	orders := makeOrders(1000)
	for b.Loop() {
		var buf bytes.Buffer
		enc := gob.NewEncoder(&buf)
		for i := range orders {
			if err := enc.Encode(&orders[i]); err != nil {
				b.Fatal(err)
			}
		}
	}
}

func BenchmarkStreamDecode(b *testing.B) {
	var buf bytes.Buffer
	enc := gob.NewEncoder(&buf)
	for _, order := range makeOrders(1000) {
		if err := enc.Encode(order); err != nil {
			b.Fatal(err)
		}
	}
	data := buf.Bytes()
	b.SetBytes(int64(len(data)))
	for b.Loop() {
		dec := gob.NewDecoder(bytes.NewReader(data))
		for range 1000 {
			var order Order
			if err := dec.Decode(&order); err != nil {
				b.Fatal(err)
			}
		}
	}
}
