// Data the Rust server takes from Mattermost's Go model package, printed as JSON:
//
//	go run . > data/config_defaults.json            model.Config after SetDefaults
//	go run . access > data/config_access.json       the `access` tag of every setting
//	go run . ancillary > data/ancillary_permissions.json   SysconsoleAncillaryPermissions
//	go run . timezones > data/timezones.json         the supported timezones
//
// Run it from a copy of this directory with a go.mod that replaces github.com/mattermost/mattermost/server/public
// with the server's source (server/public); dev/run is a good place.
//
// Based on Mattermost server/public/model, Copyright (c) 2015-present Mattermost, Inc. Licensed under Apache-2.0.
package main

import (
	"encoding/json"
	"os"
	"reflect"
	"strings"

	"github.com/mattermost/mattermost/server/public/model"
	"github.com/mattermost/mattermost/server/public/shared/timezones"
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
	mode := ""
	if len(os.Args) > 1 {
		mode = os.Args[1]
	}
	switch mode {
	case "access":
		out := map[string]string{}
		walk(reflect.TypeOf(model.Config{}), "", out)
		e.Encode(out)
	case "ancillary":
		out := map[string][]string{}
		for k, ps := range model.SysconsoleAncillaryPermissions {
			for _, p := range ps {
				out[k] = append(out[k], p.Id)
			}
		}
		e.Encode(out)
	case "timezones":
		e.Encode(timezones.DefaultSupportedTimezones)
	default:
		c := &model.Config{}
		c.SetDefaults()
		e.Encode(c)
	}
}
