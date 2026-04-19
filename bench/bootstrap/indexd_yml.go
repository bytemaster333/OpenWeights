//go:build !a3probe
// +build !a3probe

package main

import (
	"bytes"
	"os"
	"text/template"
)

type indexdYMLData struct {
	RecoveryPhrase   string
	AdminPassword    string
	PostgresPassword string
	Network          string
}

// renderIndexdYML parses the template at tmplPath and writes the rendered
// output to outPath with 0600 perms (file contains admin password + recovery
// phrase — chmod is load-bearing per T-01-07-01).
func renderIndexdYML(tmplPath, outPath string, d indexdYMLData) error {
	tmpl, err := template.ParseFiles(tmplPath)
	if err != nil {
		return err
	}
	var buf bytes.Buffer
	if err := tmpl.Execute(&buf, d); err != nil {
		return err
	}
	return os.WriteFile(outPath, buf.Bytes(), 0o600)
}
