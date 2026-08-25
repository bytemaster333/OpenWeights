package main

import (
	"bufio"
	"fmt"
	"log/slog"
	"os"
	"strings"

	"github.com/charmbracelet/huh"
	"go.sia.tech/siastorage"
)

// promptOperatorValues interactively fills the three operator-specific values
// that have no safe silent default: the indexer, the recovery phrase, and the
// console admin password. TTY only — non-TTY runs (CI, scripts) rely on .env
// being pre-filled and fall back to the callers' defaults.
//
// Only values still empty in kv are prompted, so re-running the wizard against
// an existing .env doesn't re-ask what's already set. The rich huh TUI is used
// when it can start; on any failure (a terminal it can't drive) we fall back to
// plain line prompts so the wizard never crashes.
func promptOperatorValues(logger *slog.Logger, kv map[string]string, isTTY bool) {
	if !isTTY {
		return
	}
	need := operatorNeeds{
		indexer: kv["OPENWEIGHTS_INDEXER_URL"] == "",
		phrase:  kv["OPENWEIGHTS_RECOVERY_PHRASE"] == "",
		admin:   kv["OPENWEIGHTS_ADMIN_PASSWORD"] == "",
	}
	if !need.indexer && !need.phrase && !need.admin {
		return
	}
	if err := huhForm(kv, need); err != nil {
		logger.Warn("rich prompt unavailable; using plain prompts", "err", err)
		bufioFallback(logger, kv, need)
	}
}

type operatorNeeds struct{ indexer, phrase, admin bool }

// huhForm renders the charmbracelet/huh TUI (password masking, select menus,
// inline validation). Returns an error (or recovers a panic into one) if the
// terminal can't drive it, so the caller can fall back.
func huhForm(kv map[string]string, need operatorNeeds) (err error) {
	defer func() {
		if r := recover(); r != nil {
			err = fmt.Errorf("huh panicked: %v", r)
		}
	}()

	indexerChoice := "https://sia.storage"
	customIndexer := ""
	phraseChoice := "generate"
	pastedPhrase := ""
	adminPassword := ""

	var groups []*huh.Group
	if need.indexer {
		groups = append(groups,
			huh.NewGroup(
				huh.NewSelect[string]().
					Title("Indexer").
					Description("Where OpenWeights reaches Sia. The hosted option needs no wallet funding.").
					Options(
						huh.NewOption("Hosted — https://sia.storage (50 GB free)", "https://sia.storage"),
						huh.NewOption("My own indexd", "__custom__"),
					).
					Value(&indexerChoice),
			),
			huh.NewGroup(
				huh.NewInput().
					Title("Your indexd URL").
					Placeholder("http://my-indexd:9982").
					Value(&customIndexer).
					Validate(nonEmpty("enter a URL")),
			).WithHideFunc(func() bool { return indexerChoice != "__custom__" }),
		)
	}
	if need.phrase {
		groups = append(groups,
			huh.NewGroup(
				huh.NewSelect[string]().
					Title("Sia recovery phrase").
					Description("Derives your App Key. KEEP IT SAFE — losing it permanently orphans every byte you store.").
					Options(
						huh.NewOption("Generate a fresh phrase (recommended)", "generate"),
						huh.NewOption("Paste my existing BIP-39 phrase", "paste"),
					).
					Value(&phraseChoice),
			),
			huh.NewGroup(
				huh.NewInput().
					Title("BIP-39 recovery phrase").
					Placeholder("word1 word2 word3 ...").
					Value(&pastedPhrase).
					Validate(atLeast12Words),
			).WithHideFunc(func() bool { return phraseChoice != "paste" }),
		)
	}
	if need.admin {
		groups = append(groups,
			huh.NewGroup(
				huh.NewInput().
					Title("Console admin password").
					Description("For password sign-in (no GitHub OAuth app needed). Leave blank to generate one.").
					EchoMode(huh.EchoModePassword).
					Value(&adminPassword),
			),
		)
	}

	if err = huh.NewForm(groups...).Run(); err != nil {
		return err
	}

	if need.indexer {
		if indexerChoice == "__custom__" {
			kv["OPENWEIGHTS_INDEXER_URL"] = strings.TrimSpace(customIndexer)
		} else {
			kv["OPENWEIGHTS_INDEXER_URL"] = indexerChoice
		}
	}
	if need.phrase {
		if phraseChoice == "paste" {
			kv["OPENWEIGHTS_RECOVERY_PHRASE"] = strings.TrimSpace(pastedPhrase)
		} else {
			kv["OPENWEIGHTS_RECOVERY_PHRASE"] = siastorage.NewSeedPhrase()
			fmt.Println("Generated a fresh recovery phrase and saved it to .env — back that file up.")
		}
	}
	if need.admin {
		setAdminPassword(kv, adminPassword)
	}
	return nil
}

// bufioFallback drives the same three values with plain line prompts. Used when
// the huh TUI can't start (a terminal it can't render to).
func bufioFallback(logger *slog.Logger, kv map[string]string, need operatorNeeds) {
	r := bufio.NewReader(os.Stdin)
	if need.phrase {
		fmt.Fprintln(os.Stderr, "Sia recovery phrase — derives your App Key. KEEP IT SAFE.")
		fmt.Fprint(os.Stderr, "  paste an existing BIP-39 phrase, or press Enter to generate one: ")
		if line := readLine(r); line != "" {
			kv["OPENWEIGHTS_RECOVERY_PHRASE"] = line
		} else {
			kv["OPENWEIGHTS_RECOVERY_PHRASE"] = siastorage.NewSeedPhrase()
			fmt.Fprintln(os.Stderr, "  generated a fresh phrase and saved it to .env — back that file up.")
			logger.Info("generated BIP-39 phrase", "phrase_sha_prefix", sha256Prefix8(kv["OPENWEIGHTS_RECOVERY_PHRASE"]))
		}
	}
	if need.indexer {
		fmt.Fprint(os.Stderr, "Indexer URL — Enter for https://sia.storage, or paste your own indexd URL: ")
		if line := readLine(r); line != "" {
			kv["OPENWEIGHTS_INDEXER_URL"] = line
		} else {
			kv["OPENWEIGHTS_INDEXER_URL"] = "https://sia.storage"
		}
	}
	if need.admin {
		fmt.Fprint(os.Stderr, "Console admin password — set one for password sign-in, or Enter to generate: ")
		setAdminPassword(kv, readLine(r))
	}
}

// setAdminPassword stores a typed password, or generates one and prints it once.
func setAdminPassword(kv map[string]string, typed string) {
	if strings.TrimSpace(typed) != "" {
		kv["OPENWEIGHTS_ADMIN_PASSWORD"] = typed
		return
	}
	pw := generateRandomPassword()[:24]
	kv["OPENWEIGHTS_ADMIN_PASSWORD"] = pw
	fmt.Printf("Generated admin password (saved to .env): %s\n", pw)
}

func nonEmpty(msg string) func(string) error {
	return func(s string) error {
		if strings.TrimSpace(s) == "" {
			return fmt.Errorf("%s", msg)
		}
		return nil
	}
}

func atLeast12Words(s string) error {
	if len(strings.Fields(s)) < 12 {
		return fmt.Errorf("a BIP-39 phrase is at least 12 words")
	}
	return nil
}

// readLine reads one trimmed line from the reader; a read error yields "".
func readLine(r *bufio.Reader) string {
	line, _ := r.ReadString('\n')
	return strings.TrimSpace(line)
}
