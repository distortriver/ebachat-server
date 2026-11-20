// Command emailtpl dumps the server's email templates (server/templates/*.html, Apache-2.0) after Go's
// html/template has escaped them: every action carries the escaper of its context and HTML/CSS comments are
// elided, exactly as Go renders them. The Rust server interprets this dump (src/email_template.rs), so its
// emails are byte-identical to Go's without a contextual escaper of its own.
//
//	Usage: go run . -templates ../../../ebac-messenger/mattermost/server/templates -only $(cat ../../data/email_templates.list) \
//		> ../../data/email_templates.txt
package main

import (
	"flag"
	"fmt"
	"html/template"
	"io"
	"os"
	"path/filepath"
	"sort"
	"strings"
)

type data struct {
	Props map[string]any
	HTML  map[string]template.HTML
}

func main() {
	dir := flag.String("templates", "", "the server's templates directory")
	only := flag.String("only", "", "comma-separated templates to dump (with those they call); all when empty")
	flag.Parse()
	keep := map[string]bool{}
	for _, n := range strings.Split(*only, ",") {
		if n != "" {
			keep[n] = true
		}
	}
	t, err := template.ParseGlob(filepath.Join(*dir, "*.html"))
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
	var names []string
	for _, x := range t.Templates() {
		if !strings.HasSuffix(x.Name(), ".html") {
			names = append(names, x.Name())
		}
	}
	sort.Strings(names)
	// Escaping happens on the first execution; errors of the dummy data do not matter.
	for _, n := range names {
		_ = t.ExecuteTemplate(io.Discard, n, data{Props: map[string]any{}, HTML: map[string]template.HTML{}})
	}
	var all []string
	for _, x := range t.Templates() {
		if !strings.HasSuffix(x.Name(), ".html") && x.Tree != nil && (len(keep) == 0 || keep[x.Name()]) {
			all = append(all, x.Name())
		}
	}
	sort.Strings(all)
	fmt.Println("{{/* server/templates/*.html of Mattermost 12.1.0, Copyright (c) 2015-present Mattermost, Inc., Apache-2.0; escaped by html/template (tools/emailtpl) */}}")
	for _, n := range all {
		fmt.Printf("{{define %q}}%s{{end}}\n", n, t.Lookup(n).Tree.Root.String())
	}
}
