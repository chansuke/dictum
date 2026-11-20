// The cases in tests/conformance against Go expr itself, so ported cases provably
// keep Go's result. Run with `just conformance-go`.
package bench

import (
	"fmt"
	"path/filepath"
	"reflect"
	"sort"
	"testing"
	"time"

	"github.com/BurntSushi/toml"
	"github.com/expr-lang/expr"
)

type conformanceCase struct {
	Name      string
	Expr      string
	Expect    any
	Nil       bool
	Error     string
	Deviation string
}

var sampleNow = time.Date(2026, 1, 1, 0, 0, 0, 0, time.UTC)

// dictum has no local zone: date() reads a time without an offset as UTC. Go reads it
// in time.Local, so the oracle sets that to UTC. expr.Timezone would do the same for
// date(s) but also prepends a location to calls that already pass one.
func init() { time.Local = time.UTC }

func runGo(src string) (any, error) {
	p, err := expr.Compile(src,
		expr.Env(Facts{}),
		expr.Function("now", func(args ...any) (any, error) {
			if len(args) == 1 {
				return sampleNow.In(args[0].(*time.Location)), nil
			}
			return sampleNow, nil
		}, new(func() time.Time), new(func(*time.Location) time.Time)),
	)
	if err != nil {
		return nil, err
	}
	return expr.Run(p, sample())
}

// normalize maps a Go result onto the shapes BurntSushi/toml decodes into, so a
// result compares with a case's `expect` by reflect.DeepEqual. Types it does not
// know (time.Time, structs) are kept and then never equal a TOML value.
func normalize(v any) any {
	if v == nil {
		return nil
	}
	rv := reflect.ValueOf(v)
	switch rv.Kind() {
	case reflect.Int, reflect.Int8, reflect.Int16, reflect.Int32, reflect.Int64:
		return rv.Int()
	case reflect.Uint, reflect.Uint8, reflect.Uint16, reflect.Uint32, reflect.Uint64:
		return int64(rv.Uint())
	case reflect.Float32, reflect.Float64:
		return rv.Float()
	case reflect.Pointer, reflect.Interface:
		if rv.IsNil() {
			return nil
		}
		return normalize(rv.Elem().Interface())
	case reflect.Slice, reflect.Array:
		// A nil slice is Go's nil, not an empty array: toJSON tells them apart.
		if rv.Kind() == reflect.Slice && rv.IsNil() {
			return nil
		}
		out := make([]any, rv.Len())
		for i := range out {
			out[i] = normalize(rv.Index(i).Interface())
		}
		return out
	case reflect.Map:
		// groupBy builds a map[any][]any; its keys are strings when dictum's are.
		out := make(map[string]any, rv.Len())
		for it := rv.MapRange(); it.Next(); {
			key := it.Key()
			if key.Kind() == reflect.Interface {
				key = key.Elem()
			}
			if key.Kind() != reflect.String {
				return v
			}
			out[key.String()] = normalize(it.Value().Interface())
		}
		return out
	}
	return v
}

func agrees(c conformanceCase, got any, err error) bool {
	if c.Error != "" {
		return err != nil
	}
	if err != nil {
		return false
	}
	var want any
	if !c.Nil {
		want = normalize(c.Expect)
	}
	return reflect.DeepEqual(normalize(got), want)
}

// alwaysAgrees reruns a case, because Go's result can differ between runs where it
// iterates a map, and one run that happens to match dictum does not make the note stale.
func alwaysAgrees(c conformanceCase) bool {
	for range 1000 {
		got, err := runGo(c.Expr)
		if !agrees(c, got, err) {
			return false
		}
	}
	return true
}

func expected(c conformanceCase) string {
	switch {
	case c.Error != "":
		return "error " + c.Error
	case c.Nil:
		return "nil"
	}
	return fmt.Sprintf("%#v", c.Expect)
}

func TestConformance(t *testing.T) {
	paths, err := filepath.Glob("../../../../tests/conformance/*.toml")
	if err != nil || len(paths) == 0 {
		t.Fatalf("no case files: %v", err)
	}
	sort.Strings(paths)
	var run, passed, deviations, stale int
	for _, path := range paths {
		var file struct{ Case []conformanceCase }
		if _, err := toml.DecodeFile(path, &file); err != nil {
			t.Fatalf("%s: %v", path, err)
		}
		for _, c := range file.Case {
			got, err := runGo(c.Expr)
			outcome := fmt.Sprintf("%#v", got)
			if err != nil {
				outcome = "error: " + err.Error()
			}
			id := fmt.Sprintf("%s: %s", filepath.Base(path), c.Name)
			ok := agrees(c, got, err)
			if c.Deviation != "" {
				deviations++
				// Go failing where dictum fails does not make an error deviation stale:
				// the note is usually about a different error, or other Go options.
				if ok && c.Error == "" && alwaysAgrees(c) {
					stale++
					t.Errorf("%s\n  expr: %s\n  Go agrees with dictum (%s), so the deviation is stale: %s",
						id, c.Expr, outcome, c.Deviation)
				}
				continue
			}
			run++
			if ok {
				passed++
			} else {
				t.Errorf("%s\n  expr: %s\n  expected: %s\n  Go: %s", id, c.Expr, expected(c), outcome)
			}
		}
	}
	t.Logf("%d cases checked against Go, %d agree, %d differ; %d deviations, %d stale",
		run, passed, run-passed, deviations, stale)
}
