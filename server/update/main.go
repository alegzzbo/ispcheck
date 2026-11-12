// Утилита выпуска обновлений (запускается у вас, не у клиентов).
//
//	go run ./tools/release keygen
//	go run ./tools/release add 1.0.1 windows-amd64 ispcheck.exe https://support.example.com/update/ispcheck-1.0.1.exe "что нового"
//
// После add загрузите на сервер exe и получившийся latest.json.
package main

import (
	"crypto/ed25519"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"os"
	"strings"
)

type file struct {
	URL    string `json:"url"`
	SHA256 string `json:"sha256"`
	Sig    string `json:"sig"`
}

type info struct {
	Version string          `json:"version"`
	Notes   string          `json:"notes"`
	Files   map[string]file `json:"files"`
}

func must[T any](v T, err error) T {
	if err != nil {
		fmt.Println("ошибка:", err)
		os.Exit(1)
	}
	return v
}

func main() {
	if len(os.Args) < 2 {
		fmt.Println("команды: keygen | add <version> <os-arch> <exe> <url> [notes]")
		return
	}
	switch os.Args[1] {
	case "keygen":
		pub, priv := func() (ed25519.PublicKey, ed25519.PrivateKey) {
			p, k, err := ed25519.GenerateKey(nil)
			return p, must(k, err)
		}()
		must(0, os.WriteFile("update.key", []byte(base64.StdEncoding.EncodeToString(priv)), 0o600))
		fmt.Println("Приватный ключ: update.key (храните в секрете, не коммитьте!)")
		fmt.Println("updatePubKey =", base64.StdEncoding.EncodeToString(pub), " <- вставьте в config.go")
	case "add":
		if len(os.Args) < 6 {
			fmt.Println("usage: add <version> <os-arch> <exe> <url> [notes]")
			os.Exit(1)
		}
		ver, key, exe, url := os.Args[2], os.Args[3], os.Args[4], os.Args[5]
		notes := ""
		if len(os.Args) > 6 {
			notes = os.Args[6]
		}
		sum := sha256.Sum256(must(os.ReadFile(exe)))
		priv := must(base64.StdEncoding.DecodeString(strings.TrimSpace(string(must(os.ReadFile("update.key"))))))
		sig := ed25519.Sign(ed25519.PrivateKey(priv), sum[:])

		cur := info{Files: map[string]file{}}
		if b, err := os.ReadFile("latest.json"); err == nil {
			var old info
			if json.Unmarshal(b, &old) == nil && old.Version == ver && old.Files != nil {
				cur = old
			}
		}
		cur.Version, cur.Notes = ver, notes
		cur.Files[key] = file{URL: url, SHA256: hex.EncodeToString(sum[:]), Sig: base64.StdEncoding.EncodeToString(sig)}
		out := must(json.MarshalIndent(cur, "", "  "))
		must(0, os.WriteFile("latest.json", out, 0o644))
		fmt.Println("latest.json обновлён:", ver, key)
	}
}
