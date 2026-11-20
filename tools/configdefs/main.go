package main

import (
	"encoding/json"
	"os"
	"reflect"
	"strings"

	"github.com/mattermost/mattermost/server/public/model"
)

// walk records the `access` tag of every leaf setting, by its JSON path.
func walk(t reflect.Type, prefix string, out map[string]string) {
	for i := 0; i < t.NumField(); i++ {
		f := t.Field(i)
		name := strings.Split(f.Tag.Get("json"), ",")[0]
		if name == "-" {
			continue
		}
		if name == "" {
			name = f.Name
		}
		ft := f.Type
		if ft.Kind() == reflect.Ptr {
			ft = ft.Elem()
		}
		if ft.Kind() == reflect.Struct {
			walk(ft, prefix+name+".", out)
			continue
		}
		out[prefix+name] = f.Tag.Get("access")
	}
}

func main() {
	e := json.NewEncoder(os.Stdout)
	e.SetIndent("", "    ")
	if len(os.Args) > 1 && os.Args[1] == "access" {
		out := map[string]string{}
		walk(reflect.TypeOf(model.Config{}), "", out)
		e.Encode(out)
		return
	}
	c := &model.Config{}
	c.SetDefaults()
	e.Encode(c)
}
