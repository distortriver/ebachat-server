// mmmap builds a map of the Mattermost server's REST API from its source: every route in channels/api4 with its
// auth wrapper, permissions, the app and store methods it reaches, the tables and websocket events behind it, and
// whether EBAChat or production traffic uses it. Only the standard library: go/ast, no type checking, so the call
// graph is an approximation (calls are resolved by receiver type and method name).
//
//	go run ./tools/mmmap -mm ../ebac-messenger/mattermost/server -client ../ebac-messenger/src -out docs/map
package main

import (
	"bufio"
	"flag"
	"fmt"
	"go/ast"
	"go/parser"
	"go/token"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strconv"
	"strings"
)

var fset = token.NewFileSet()

type link struct {
	name string
	args []ast.Expr
	call bool
}

// chain flattens `a.b.c(x).d()` into [a b c(x) d()].
func chain(e ast.Expr) []link {
	switch v := e.(type) {
	case *ast.Ident:
		return []link{{name: v.Name}}
	case *ast.SelectorExpr:
		return append(chain(v.X), link{name: v.Sel.Name})
	case *ast.CallExpr:
		c := chain(v.Fun)
		if len(c) == 0 {
			return nil
		}
		c[len(c)-1].call, c[len(c)-1].args = true, v.Args
		return c
	case *ast.ParenExpr:
		return chain(v.X)
	case *ast.IndexExpr:
		return chain(v.X)
	}
	return nil
}

func names(c []link) []string {
	out := make([]string, len(c))
	for i, l := range c {
		out[i] = l.name
	}
	return out
}

func parseDir(dir string, recursive bool) []*ast.File {
	var files []*ast.File
	filepath.WalkDir(dir, func(p string, d os.DirEntry, err error) error {
		if err != nil {
			return nil
		}
		if d.IsDir() {
			if p != dir && (!recursive || d.Name() == "mocks" || d.Name() == "testlib") {
				return filepath.SkipDir
			}
			return nil
		}
		if !strings.HasSuffix(p, ".go") || strings.HasSuffix(p, "_test.go") {
			return nil
		}
		f, err := parser.ParseFile(fset, p, nil, parser.SkipObjectResolution)
		if err != nil {
			fmt.Fprintln(os.Stderr, "parse:", err)
			return nil
		}
		files = append(files, f)
		return nil
	})
	return files
}

func strLit(e ast.Expr) (string, bool) {
	if b, ok := e.(*ast.BasicLit); ok && b.Kind == token.STRING {
		s, err := strconv.Unquote(b.Value)
		return s, err == nil
	}
	return "", false
}

// cleanPath turns gorilla templates `{id:[a-z]+}` into `{id}`.
func cleanPath(p string) string {
	var b strings.Builder
	for i := 0; i < len(p); i++ {
		if p[i] != '{' {
			b.WriteByte(p[i])
			continue
		}
		depth, j := 0, i
		for ; j < len(p); j++ {
			if p[j] == '{' {
				depth++
			} else if p[j] == '}' {
				if depth--; depth == 0 {
					break
				}
			}
		}
		inner := p[i+1 : j]
		if k := strings.IndexByte(inner, ':'); k >= 0 {
			inner = inner[:k]
		}
		b.WriteString("{" + inner + "}")
		i = j
	}
	return b.String()
}

func pos(n ast.Node) string {
	p := fset.Position(n.Pos())
	return fmt.Sprintf("%s:%d", p.Filename, p.Line)
}

func lines(n ast.Node) int { return fset.Position(n.End()).Line - fset.Position(n.Pos()).Line + 1 }

// ---------- functions and the call graph ----------

type fn struct {
	key   string // "App.CreatePost", "store:Post.Save", "api:createPost"
	decl  *ast.FuncDecl
	calls []string
	perms map[string]bool
	ws    map[string]bool
	lits  []string
}

var funcs = map[string]*fn{}

func recvType(d *ast.FuncDecl) (typ, varName string) {
	if d.Recv == nil || len(d.Recv.List) == 0 {
		return "", ""
	}
	f := d.Recv.List[0]
	t := f.Type
	if s, ok := t.(*ast.StarExpr); ok {
		t = s.X
	}
	if id, ok := t.(*ast.Ident); ok {
		typ = id.Name
	}
	if len(f.Names) > 0 {
		varName = f.Names[0].Name
	}
	return
}

var storeTypeRe = regexp.MustCompile(`^Sql(\w+)Store$`)

// appKey maps a receiver chain in app code to a type we index.
var appFields = map[string]string{"ch": "Channels", "srv": "Server", "platform": "PlatformService"}

func analyze(kind string, d *ast.FuncDecl, key string) {
	typ, rv := recvType(d)
	f := &fn{key: key, decl: d, perms: map[string]bool{}, ws: map[string]bool{}}
	funcs[key] = f
	if d.Body == nil {
		return
	}
	ast.Inspect(d.Body, func(n ast.Node) bool {
		switch v := n.(type) {
		case *ast.SelectorExpr:
			if x, ok := v.X.(*ast.Ident); ok && x.Name == "model" {
				if strings.HasPrefix(v.Sel.Name, "Permission") && v.Sel.Name != "Permission" {
					f.perms[strings.TrimPrefix(v.Sel.Name, "Permission")] = true
				}
				if strings.HasPrefix(v.Sel.Name, "WebsocketEvent") {
					f.ws[v.Sel.Name] = true
				}
			}
		case *ast.BasicLit:
			if v.Kind == token.STRING {
				f.lits = append(f.lits, v.Value)
			}
		case *ast.CallExpr:
			c := chain(v)
			if len(c) == 0 {
				return true
			}
			n := names(c)
			// store calls anywhere: ...Store().Entity().Method(
			for i := 0; i+2 < len(c); i++ {
				if c[i].name == "Store" && c[i].call && c[i+1].call && c[i+2].call {
					f.calls = append(f.calls, "store:"+c[i+1].name+"."+c[i+2].name)
				}
			}
			switch kind {
			case "api":
				if len(n) == 1 {
					f.calls = append(f.calls, "api:"+n[0])
				}
				if len(n) >= 3 && n[0] == "c" && n[1] == "App" {
					if c[2].call && len(n) == 3 {
						f.calls = append(f.calls, "App."+n[2])
					} else if len(n) == 4 && c[2].call && n[2] == "Channels" {
						f.calls = append(f.calls, "Channels."+n[3])
					}
				}
			case "app":
				if len(n) == 2 && n[0] == rv {
					f.calls = append(f.calls, typ+"."+n[1])
				}
				if len(n) == 3 && n[0] == rv {
					if t, ok := appFields[n[1]]; ok {
						f.calls = append(f.calls, t+"."+n[2])
					}
					if n[1] == "Srv" && c[1].call {
						f.calls = append(f.calls, "Server."+n[2])
					}
				}
				if len(n) == 1 {
					f.calls = append(f.calls, "appfn:"+n[0])
				}
			case "store":
				if len(n) == 2 && n[0] == rv {
					f.calls = append(f.calls, "store:"+strings.TrimSuffix(storeTypeRe.ReplaceAllString(typ, "$1"), "")+"."+n[1])
				}
				if len(n) == 1 {
					f.calls = append(f.calls, "storefn:"+n[0])
				}
			}
		}
		return true
	})
}

type reach struct {
	keys          map[string]bool
	perms, ws     map[string]bool
	store, tables map[string]bool
	loc           int
}

var tableNames []string
var tableRe *regexp.Regexp

func walk(key string, r *reach, depth int) {
	if r.keys[key] || depth > 12 {
		return
	}
	f := funcs[key]
	if f == nil {
		return
	}
	r.keys[key] = true
	r.loc += lines(f.decl)
	for p := range f.perms {
		r.perms[p] = true
	}
	for w := range f.ws {
		r.ws[w] = true
	}
	if strings.HasPrefix(key, "store:") || strings.HasPrefix(key, "storefn:") {
		if strings.HasPrefix(key, "store:") {
			r.store[strings.TrimPrefix(key, "store:")] = true
		}
		for _, l := range f.lits {
			for _, m := range tableRe.FindAllString(l, -1) {
				r.tables[strings.ToLower(m)] = true
			}
		}
	}
	for _, c := range f.calls {
		walk(c, r, depth+1)
	}
}

// ---------- routes ----------

type route struct {
	method, path, wrapper, handler, at string
	r                                  *reach
	ebachat                            []string
	prod                               int64
	area, tier                         string
}

func collectRoutes(api []*ast.File) ([]*route, int) {
	prefix := map[string]string{"Root": "", "APIRoot": "/api/v4", "APIRoot5": "/api/v5"}
	type pending struct{ x, y, p string }
	var defs []pending
	for _, f := range api {
		ast.Inspect(f, func(n ast.Node) bool {
			a, ok := n.(*ast.AssignStmt)
			if !ok || len(a.Lhs) != 1 {
				return true
			}
			l := names(chain(a.Lhs[0]))
			if len(l) != 3 || l[1] != "BaseRoutes" {
				return true
			}
			rc := chain(a.Rhs[0])
			for i, k := range rc {
				if k.name == "PathPrefix" && i >= 1 {
					p, _ := strLit(k.args[0])
					base := rc[i-1].name
					if rc[i-1].name == "Router" {
						base = "Root"
					}
					defs = append(defs, pending{l[2], base, p})
				}
			}
			return true
		})
	}
	for changed := true; changed; {
		changed = false
		for _, d := range defs {
			if b, ok := prefix[d.y]; ok {
				if _, done := prefix[d.x]; !done {
					prefix[d.x] = b + d.p
					changed = true
				}
			}
		}
	}

	var routes []*route
	local := 0
	seen := map[*ast.CallExpr]bool{}
	for _, f := range api {
		ast.Inspect(f, func(n ast.Node) bool {
			call, ok := n.(*ast.CallExpr)
			if !ok || seen[call] {
				return true
			}
			c := chain(call)
			// [api BaseRoutes X Handle(...) Methods(...)?]
			if len(c) < 4 || c[0].name != "api" || c[1].name != "BaseRoutes" || c[3].name != "Handle" {
				return true
			}
			if len(c) > 5 || (len(c) == 5 && c[4].name != "Methods") {
				return true
			}
			// mark inner Handle call as seen
			if inner, ok := call.Fun.(*ast.SelectorExpr); ok && len(c) == 5 {
				if ic, ok := inner.X.(*ast.CallExpr); ok {
					seen[ic] = true
				}
			}
			base, ok := prefix[c[2].name]
			if !ok {
				fmt.Fprintln(os.Stderr, "unknown base", c[2].name, pos(call))
				return true
			}
			p, _ := strLit(c[3].args[0])
			var wrappers []string
			handler := ""
			var h ast.Expr = c[3].args[1]
			for handler == "" {
				hc, ok := h.(*ast.CallExpr)
				if !ok {
					if id, ok := h.(*ast.Ident); ok {
						handler = id.Name
					} else {
						handler = "?"
					}
					break
				}
				hn := names(chain(hc.Fun))
				wrappers = append(wrappers, hn[len(hn)-1])
				if len(hc.Args) == 0 {
					handler = "?"
					break
				}
				h = hc.Args[0]
			}
			if len(wrappers) > 0 && wrappers[0] == "APILocal" {
				local++
				return true
			}
			methods := []string{"ANY"}
			if len(c) == 5 {
				methods = nil
				for _, a := range c[4].args {
					if s, ok := strLit(a); ok {
						methods = append(methods, s)
					} else if sel, ok := a.(*ast.SelectorExpr); ok {
						methods = append(methods, strings.ToUpper(strings.TrimPrefix(sel.Sel.Name, "Method")))
					}
				}
			}
			for _, m := range methods {
				routes = append(routes, &route{method: m, path: strings.Replace(cleanPath(base+p), "{websocket}", "websocket", 1), wrapper: strings.Join(wrappers, "+"),
					handler: handler, at: pos(call)})
			}
			return false
		})
	}
	return routes, local
}

// ---------- usage: EBAChat client and production traffic ----------

type use struct {
	method, path, where string
	count               int64
}

var (
	clientPathRe   = regexp.MustCompile(`"(?:\{\}/api/v4)?(/[a-z][^"\s]*)"`)
	clientMethodRe = regexp.MustCompile(`\.(get|post|put|delete|patch)[a-z_]*\b|"(GET|POST|PUT|DELETE|PATCH)"`)
	clientArmRe    = regexp.MustCompile(`"([a-z_]+)"\s*(?:\||=>)`)
	// Paths the client builds from a variable part that is itself a path.
	clientExpand = map[string][]string{
		"/posts/{post}/{action}": {"/posts/{post}/pin", "/posts/{post}/unpin"},
		"/teams/{team}/{path}":   {"/teams/{team}/channels", "/teams/{team}/channels/deleted"},
		"/users{query}":          {"/users"},
		"/users/{me}/channels/{channel}/drafts{root}": {"/users/{me}/channels/{channel}/drafts",
			"/users/{me}/channels/{channel}/drafts/{root}"},
	}
)

// clientUses finds every API path literal in the EBAChat source. The method is read from the nearest line around
// it that names one (`self.put_ok(`, `.post(`, `"DELETE"`); a path with none nearby is a download, so GET.
func clientUses(dir string, firstSegs map[string]bool, wsValues map[string]bool) (uses []use, wsHandled map[string]bool) {
	wsHandled = map[string]bool{}
	filepath.WalkDir(dir, func(p string, d os.DirEntry, err error) error {
		if err != nil || d.IsDir() || !strings.HasSuffix(p, ".rs") {
			return nil
		}
		b, _ := os.ReadFile(p)
		ls := strings.Split(string(b), "\n")
		methodsAt := func(i int) []string {
			if i < 0 || i >= len(ls) {
				return nil
			}
			var ms []string
			for _, m := range clientMethodRe.FindAllStringSubmatch(ls[i], -1) {
				ms = append(ms, strings.ToUpper(m[1]+m[2]))
			}
			return ms
		}
		for i, l := range ls {
			t := strings.TrimSpace(l)
			if strings.HasPrefix(t, "//") {
				continue
			}
			for _, m := range clientArmRe.FindAllStringSubmatch(l, -1) {
				if wsValues[m[1]] {
					wsHandled[m[1]] = true
				}
			}
			for _, m := range clientPathRe.FindAllStringSubmatch(l, -1) {
				path := m[1]
				first := strings.Split(strings.Trim(strings.SplitN(path, "?", 2)[0], "/"), "/")[0]
				if i := strings.IndexByte(first, '{'); i >= 0 {
					first = first[:i]
				}
				if !firstSegs[first] {
					continue
				}
				var ms []string
				for _, off := range []int{0, 1, -1, 2, -2, 3, 4} {
					if ms = methodsAt(i + off); len(ms) > 0 {
						break
					}
				}
				if len(ms) == 0 {
					ms = []string{"GET"}
				}
				paths := []string{path}
				if x, ok := clientExpand[strings.SplitN(path, "?", 2)[0]]; ok {
					paths = x
				}
				for _, pp := range paths {
					for _, mm := range uniq(ms) {
						uses = append(uses, use{method: mm, path: pp, where: fmt.Sprintf("%s:%d", strings.TrimPrefix(p, dir+"/"), i+1)})
					}
				}
			}
		}
		return nil
	})
	return
}

// wsEventValues reads `WebsocketEventPosted WebsocketEventType = "posted"` from the model.
func wsEventValues(mm string) map[string]string {
	b, _ := os.ReadFile(filepath.Join(mm, "public/model/websocket_message.go"))
	out := map[string]string{}
	for _, m := range regexp.MustCompile(`(WebsocketEvent\w+)\s+WebsocketEventType\s*=\s*"([^"]+)"`).FindAllStringSubmatch(string(b), -1) {
		out[m[1]] = m[2]
	}
	return out
}

var placeholderRe = regexp.MustCompile(`\{[^}]*\}`)

// segs normalises a concrete or templated path into segments; "*" matches anything.
func segs(p string) []string {
	if i := strings.IndexByte(p, '?'); i >= 0 {
		p = p[:i]
	}
	p = strings.TrimPrefix(p, "/api/v4")
	p = placeholderRe.ReplaceAllString(p, "*")
	p = strings.Trim(p, "/")
	if p == "" {
		return nil
	}
	return strings.Split(p, "/")
}

// match scores how well a usage path matches a route template; -1 = no match.
func match(route, usage []string, greedyLast bool) int {
	if len(usage) != len(route) && !(greedyLast && len(usage) >= len(route)) {
		return -1
	}
	score := 0
	for i, r := range route {
		u := usage[i]
		switch {
		case r == "*":
		case u == "*" || strings.Contains(u, "*"):
			if !strings.HasPrefix(u, strings.TrimSuffix(u, "*")) || (u != "*" && !strings.HasPrefix(r, strings.TrimSuffix(u, "*"))) {
				return -1
			}
		case u == r:
			score += 2
		default:
			return -1
		}
	}
	return score
}

func bestRoutes(routes []*route, u use) []*route {
	us := segs(u.path)
	best, bestScore := []*route(nil), -1
	for _, r := range routes {
		if u.method != "" && u.method != "?" && r.method != u.method && r.method != "ANY" {
			continue
		}
		rp := r.path
		if !strings.HasPrefix(rp, "/api/v4") {
			continue
		}
		greedy := strings.HasSuffix(rp, "{email}") || strings.Contains(rp, "{anything}")
		s := match(segs(rp), us, greedy)
		if s > bestScore {
			best, bestScore = []*route{r}, s
		} else if s == bestScore && s >= 0 {
			best = append(best, r)
		}
	}
	return best
}

func prodUses(path string) []use {
	fh, err := os.Open(path)
	if err != nil {
		return nil
	}
	defer fh.Close()
	var out []use
	sc := bufio.NewScanner(fh)
	for sc.Scan() {
		f := strings.Split(sc.Text(), "\t")
		if len(f) < 3 || f[0] == "method" {
			continue
		}
		// count_4h_observed (last column) when present: real counts, not the 7-day estimate.
		n, _ := strconv.ParseInt(f[len(f)-1], 10, 64)
		out = append(out, use{method: strings.ToUpper(f[0]), path: f[1], count: n})
	}
	return out
}

// ---------- tiers ----------

// Areas we do not port: enterprise-only, cloud, cluster, plugins and the like. Decided from the area name only.
var skipAreas = map[string]bool{
	"saml": true, "ldap": true, "compliance": true, "cluster": true, "elasticsearch": true, "data_retention": true,
	"cloud": true, "hosted_customer": true, "remotecluster": true, "sharedchannels": true, "ip_filtering": true,
	"access_control_policies": true, "access_control": true, "content_flagging": true, "agents": true,
	"llmservices": true, "recaps": true, "scheduled_recaps": true, "boards": true, "audit_logs": true,
	"outgoing_oauth_connections": true, "custom_profile_attributes": true, "properties": true, "imports": true,
	"exports": true, "usage": true, "limits": true, "reports": true, "plugins": true, "ephemeral_mode": true,
	"delivery_tracking": true, "health": true, "jobs": true, "groups": true, "permissions": true, "brand": true,
}

func area(p string) string {
	s := strings.Split(strings.TrimPrefix(p, "/api/v4/"), "/")
	if !strings.HasPrefix(p, "/api/v4") {
		return "(root)"
	}
	if s[0] == "oauth" && len(s) > 1 && s[1] == "outgoing_connections" {
		return "outgoing_oauth_connections"
	}
	if s[0] == "users" && len(s) > 2 && s[1] == "{user_id}" {
		switch s[2] {
		case "teams":
			if len(s) > 4 && s[4] == "threads" {
				return "threads"
			}
			if len(s) > 4 && s[4] == "channels" {
				return "channels"
			}
			if len(s) > 4 && s[4] == "drafts" {
				return "drafts"
			}
			return "teams"
		case "channels":
			if len(s) > 4 && s[4] == "drafts" {
				return "drafts"
			}
			return "channels"
		case "posts":
			return "posts"
		case "preferences":
			return "preferences"
		case "status":
			return "status"
		}
	}
	return s[0]
}

// ---------- output ----------

func keys(m map[string]bool) string {
	var s []string
	for k := range m {
		s = append(s, k)
	}
	sort.Strings(s)
	return strings.Join(s, ",")
}

func main() {
	mm := flag.String("mm", "../ebac-messenger/mattermost/server", "Mattermost server source")
	client := flag.String("client", "../ebac-messenger/src", "EBAChat client source")
	traffic := flag.String("traffic", "docs/prod-traffic.tsv", "production traffic TSV (method, path, count)")
	migrations := flag.String("migrations", "migrations/postgres", "schema migrations")
	out := flag.String("out", "docs/map", "output directory")
	flag.Parse()

	// tables from the schema
	tset := map[string]bool{}
	ms, _ := filepath.Glob(filepath.Join(*migrations, "*.up.sql"))
	ctRe := regexp.MustCompile(`(?i)create table (?:if not exists )?([a-z_]+)`)
	for _, m := range ms {
		b, _ := os.ReadFile(m)
		for _, x := range ctRe.FindAllStringSubmatch(string(b), -1) {
			tset[strings.ToLower(x[1])] = true
		}
	}
	for t := range tset {
		tableNames = append(tableNames, t)
	}
	sort.Slice(tableNames, func(i, j int) bool { return len(tableNames[i]) > len(tableNames[j]) })
	tableRe = regexp.MustCompile(`(?i)\b(` + strings.Join(tableNames, "|") + `)\b`)

	api := parseDir(filepath.Join(*mm, "channels/api4"), false)
	for _, f := range api {
		for _, d := range f.Decls {
			if fd, ok := d.(*ast.FuncDecl); ok && fd.Recv == nil {
				analyze("api", fd, "api:"+fd.Name.Name)
			}
		}
	}
	for _, f := range parseDir(filepath.Join(*mm, "channels/app"), true) {
		for _, d := range f.Decls {
			if fd, ok := d.(*ast.FuncDecl); ok {
				if t, _ := recvType(fd); t != "" {
					analyze("app", fd, t+"."+fd.Name.Name)
				} else if f.Name.Name == "app" {
					analyze("app", fd, "appfn:"+fd.Name.Name)
				}
			}
		}
	}
	for _, f := range parseDir(filepath.Join(*mm, "channels/store/sqlstore"), false) {
		for _, d := range f.Decls {
			if fd, ok := d.(*ast.FuncDecl); ok {
				t, _ := recvType(fd)
				if m := storeTypeRe.FindStringSubmatch(t); m != nil {
					analyze("store", fd, "store:"+m[1]+"."+fd.Name.Name)
				} else if t == "" {
					analyze("store", fd, "storefn:"+fd.Name.Name)
				}
			}
		}
	}

	routes, local := collectRoutes(api)
	for _, r := range routes {
		r.r = &reach{keys: map[string]bool{}, perms: map[string]bool{}, ws: map[string]bool{}, store: map[string]bool{}, tables: map[string]bool{}}
		walk("api:"+r.handler, r.r, 0)
		r.area = area(r.path)
	}

	firstSegs := map[string]bool{}
	for _, r := range routes {
		if s := segs(r.path); len(s) > 0 && strings.HasPrefix(r.path, "/api/v4") {
			firstSegs[s[0]] = true
		}
	}
	firstSegs["websocket"] = true
	wsConst := wsEventValues(*mm)
	wsVals := map[string]bool{}
	for _, v := range wsConst {
		wsVals[v] = true
	}
	cu, wsHandled := clientUses(*client, firstSegs, wsVals)
	var unmatchedClient []use
	for _, u := range cu {
		b := bestRoutes(routes, u)
		if len(b) == 0 {
			unmatchedClient = append(unmatchedClient, u)
		}
		for _, r := range b {
			r.ebachat = append(r.ebachat, u.where)
		}
	}
	pu := prodUses(*traffic)
	var unmatchedProd []use
	for _, u := range pu {
		b := bestRoutes(routes, u)
		if len(b) == 0 {
			unmatchedProd = append(unmatchedProd, u)
			continue
		}
		b[0].prod += u.count
	}

	for _, r := range routes {
		switch {
		case len(r.ebachat) > 0:
			r.tier = "T1"
		case r.prod > 0 && skipAreas[r.area]:
			r.tier = "stub"
		case r.prod > 0:
			r.tier = "T2"
		case skipAreas[r.area]:
			r.tier = "skip"
		default:
			r.tier = "T3"
		}
	}
	sort.SliceStable(routes, func(i, j int) bool {
		if routes[i].path != routes[j].path {
			return routes[i].path < routes[j].path
		}
		return routes[i].method < routes[j].method
	})

	os.MkdirAll(*out, 0o755)
	rel := func(p string) string {
		if r, err := filepath.Rel(*mm, p); err == nil {
			return r
		}
		return p
	}
	w, _ := os.Create(filepath.Join(*out, "routes.tsv"))
	fmt.Fprintln(w, "tier\tarea\tmethod\tpath\tauth\thandler\tsource\tprod_count\tebachat\tpermissions\tws_events\tstore_calls\ttables\tgo_loc_reachable\trust")
	for _, r := range routes {
		fmt.Fprintf(w, "%s\t%s\t%s\t%s\t%s\t%s\t%s\t%d\t%s\t%s\t%s\t%s\t%s\t%d\t\n", r.tier, r.area, r.method, r.path,
			r.wrapper, r.handler, rel(r.at), r.prod, strings.Join(uniq(r.ebachat), ","), keys(r.r.perms),
			strings.ReplaceAll(keys(r.r.ws), "WebsocketEvent", ""), keys(r.r.store), keys(r.r.tables), r.r.loc)
	}
	w.Close()

	// websocket events: every event the model declares, which routes emit it, whether EBAChat handles it
	wsOut, _ := os.Create(filepath.Join(*out, "ws-events.tsv"))
	fmt.Fprintln(wsOut, "event\tebachat_handles\temitted_by_t1_routes\temitted_by_routes")
	evRoutes := map[string][]*route{}
	for _, r := range routes {
		for e := range r.r.ws {
			evRoutes[e] = append(evRoutes[e], r)
		}
	}
	var evs []string
	for e := range wsConst {
		evs = append(evs, e)
	}
	sort.Slice(evs, func(i, j int) bool { return wsConst[evs[i]] < wsConst[evs[j]] })
	for _, e := range evs {
		var t1, all []string
		for _, r := range evRoutes[e] {
			id := r.method + " " + strings.TrimPrefix(r.path, "/api/v4")
			all = append(all, id)
			if r.tier == "T1" {
				t1 = append(t1, id)
			}
		}
		h := ""
		if wsHandled[wsConst[e]] {
			h = "yes"
		}
		fmt.Fprintf(wsOut, "%s\t%s\t%d\t%d\n", wsConst[e], h, len(t1), len(all))
	}
	wsOut.Close()

	// summary on stdout, for docs/MAP.md
	type agg struct {
		n, t1, t2, t3, skip, loc1 int
		prod                      int64
	}
	areas := map[string]*agg{}
	tot := &agg{}
	for _, r := range routes {
		a := areas[r.area]
		if a == nil {
			a = &agg{}
			areas[r.area] = a
		}
		for _, x := range []*agg{a, tot} {
			x.n++
			x.prod += r.prod
			switch r.tier {
			case "T1":
				x.t1++
				x.loc1 += r.r.loc
			case "T2":
				x.t2++
			case "T3":
				x.t3++
			default:
				x.skip++
			}
		}
	}
	var an []string
	for k := range areas {
		an = append(an, k)
	}
	sort.Slice(an, func(i, j int) bool {
		if areas[an[i]].t1 != areas[an[j]].t1 {
			return areas[an[i]].t1 > areas[an[j]].t1
		}
		return areas[an[i]].n > areas[an[j]].n
	})
	fmt.Printf("routes %d (local-mode routes dropped: %d), T1 %d, T2 %d, T3 %d, skip %d\n", tot.n, local, tot.t1, tot.t2, tot.t3, tot.skip)
	fmt.Println("\n| area | routes | T1 | T2 | T3 | skip | prod requests |\n|---|---:|---:|---:|---:|---:|---:|")
	for _, k := range an {
		a := areas[k]
		fmt.Printf("| %s | %d | %d | %d | %d | %d | %d |\n", k, a.n, a.t1, a.t2, a.t3, a.skip, a.prod)
	}
	// distinct Go LoC behind T1 routes (union of reachable functions)
	union := map[string]bool{}
	tables := map[string]bool{}
	for _, r := range routes {
		if r.tier == "T1" {
			for k := range r.r.keys {
				union[k] = true
			}
			for t := range r.r.tables {
				tables[t] = true
			}
		}
	}
	loc := 0
	for k := range union {
		loc += lines(funcs[k].decl)
	}
	fmt.Printf("\nT1: %d distinct Go functions reachable, %d lines; %d tables touched: %s\n", len(union), loc, len(tables), keys(tables))
	fmt.Println("\nEBAChat calls with no matching route:")
	for _, u := range unmatchedClient {
		fmt.Printf("  %s %s (%s)\n", u.method, u.path, u.where)
	}
	if len(unmatchedProd) > 0 {
		fmt.Println("\nproduction paths with no matching route:")
		for _, u := range unmatchedProd {
			fmt.Printf("  %s %s %d\n", u.method, u.path, u.count)
		}
	}
}

func uniq(s []string) []string {
	m := map[string]bool{}
	var out []string
	for _, x := range s {
		if !m[x] {
			m[x] = true
			out = append(out, x)
		}
	}
	return out
}
