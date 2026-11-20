// Go expr on the rules and facts of ../eval.rs, for the comparison in
// docs/benchmarks.md. Run with `just bench-go`.
package bench

import (
	"testing"
	"time"

	"github.com/expr-lang/expr"
	"github.com/expr-lang/expr/vm"
)

type User struct {
	Name    string
	Age     int64
	Score   float64
	Active  bool
	Email   *string
	Tags    []string
	Created time.Time
	Attrs   any
	Manager *User
}

type Item struct {
	Sku   string
	Price float64
	Qty   int64
}

type Order struct {
	Total    float64
	Items    []Item
	Notes    map[string]string
	Discount *float64
}

type Facts struct {
	User  User    `expr:"user"`
	Order Order   `expr:"order"`
	Nums  []int64 `expr:"nums"`
	Limit int64   `expr:"limit"`
	Level uint8   `expr:"level"`
	Delta int8    `expr:"delta"`
}

// sample is dictum_testkit::sample().
func sample() Facts {
	discount := 0.1
	return Facts{
		User: User{
			Name: "Ada", Age: 36, Score: 4.5, Active: true,
			Tags:    []string{"admin", "beta"},
			Created: time.Unix(1_700_000_000, 0).UTC(),
			Attrs:   int64(7),
		},
		Order: Order{
			Total:    120.5,
			Items:    []Item{{Sku: "A-1", Price: 100.0, Qty: 1}, {Sku: "B-2", Price: 10.25, Qty: 2}},
			Notes:    map[string]string{"gift": "yes", "note": "leave at door"},
			Discount: &discount,
		},
		Nums:  []int64{3, 1, 4, 1, 5, 9, 2, 6},
		Limit: 10,
		Level: 200,
		Delta: -100,
	}
}

// rules is RULES in ../eval.rs, with the result each rule gives on sample().
var rules = []struct {
	name, src string
	want      bool
}{
	{"compare", "user.Age >= 18 && user.Active", true},
	{"membership", `user.Name in ["Ada", "Grace", "Barbara"] || "admin" in user.Tags`, true},
	{"predicate", "all(nums, # > 0) && count(nums, # % 2 == 0) > 2", true},
	{"strings", `upper(user.Name) startsWith "A" && len(join(user.Tags, ",")) > 3`, true},
	{"arithmetic", "sum(map(order.Items, .Price * .Qty)) * (1 - (order.Discount ?? 0.0)) > 100", true},
}

func compile(tb testing.TB, src string) *vm.Program {
	p, err := expr.Compile(src, expr.Env(Facts{}), expr.AsBool())
	if err != nil {
		tb.Fatalf("%s: %v", src, err)
	}
	return p
}

func TestRulesGiveTheExpectedResults(t *testing.T) {
	facts := sample()
	for _, r := range rules {
		out, err := expr.Run(compile(t, r.src), facts)
		if err != nil || out != r.want {
			t.Errorf("%s: got %v, %v; want %v", r.name, out, err, r.want)
		}
	}
}

func BenchmarkEval(b *testing.B) {
	facts := sample()
	for _, r := range rules {
		p := compile(b, r.src)
		b.Run(r.name, func(b *testing.B) {
			var machine vm.VM
			for i := 0; i < b.N; i++ {
				if _, err := machine.Run(p, facts); err != nil {
					b.Fatal(err)
				}
			}
		})
	}
}

func BenchmarkCompile(b *testing.B) {
	src := rules[len(rules)-1].src
	for i := 0; i < b.N; i++ {
		compile(b, src)
	}
}
