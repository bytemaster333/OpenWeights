package main

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"text/template"
)

// renderReportMarkdown loads REPORT.md.tmpl from the thesis package dir
// (we assume the program runs from repo root or from bench/) and renders
// into a markdown string.
func renderReportMarkdown(tmplPath string, r Report) (string, error) {
	tmpl, err := template.ParseFiles(tmplPath)
	if err != nil {
		return "", fmt.Errorf("parse template %s: %w", tmplPath, err)
	}
	// Template uses `{{ printf "%.2fx" .Ratio }}` on Trials — that's fine.
	var buf strBuilder
	if err := tmpl.Execute(&buf, r); err != nil {
		return "", fmt.Errorf("execute template: %w", err)
	}
	return buf.String(), nil
}

// writeRun serializes r to <dir>/report.json and returns the file path written.
func writeRun(dir string, r Report) (string, error) {
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return "", err
	}
	path := filepath.Join(dir, "report.json")
	b, err := json.MarshalIndent(r, "", "  ")
	if err != nil {
		return "", err
	}
	if err := os.WriteFile(path, b, 0o644); err != nil {
		return "", err
	}
	return path, nil
}

// writeReportMarkdown writes the rendered markdown to bench/thesis/REPORT.md.
func writeReportMarkdown(path, content string) error {
	return os.WriteFile(path, []byte(content), 0o644)
}

// strBuilder — minimal wrapper satisfying io.Writer, kept simple to avoid strings.Builder alloc variance.
type strBuilder struct{ b []byte }

func (s *strBuilder) Write(p []byte) (int, error) { s.b = append(s.b, p...); return len(p), nil }
func (s *strBuilder) String() string              { return string(s.b) }
