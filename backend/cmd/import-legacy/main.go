package main

import (
	"context"
	"encoding/json"
	"flag"
	"fmt"
	"os"
	"time"

	"github.com/jackc/pgx/v5"
	"relayai/backend/internal/legacyimport"
)

func main() {
	if err := run(); err != nil {
		fmt.Fprintln(os.Stderr, "Import failed:", err)
		os.Exit(1)
	}
}

func run() error {
	bundle := flag.String("bundle", "accounts.json", "encrypted account bundle")
	sourceKey := flag.String("source-key-file", "gateway_master-key", "source base64 master key file")
	validateOnly := flag.Bool("validate-only", false, "validate source without connecting to a database")
	apply := flag.Bool("apply", false, "commit imports; default is a read-only preview")
	flag.Parse()
	p, err := legacyimport.Load(*bundle, *sourceKey)
	if err != nil {
		return err
	}
	if *validateOnly {
		if *apply {
			return fmt.Errorf("--validate-only cannot be combined with --apply")
		}
		return json.NewEncoder(os.Stdout).Encode(map[string]any{"mode": "source-validation", "accounts": p.Count(), "decryptedCredentials": p.Count()})
	}
	key, err := legacyimport.ReadKey(os.Getenv("RELAYAI_MASTER_KEY"))
	if err != nil {
		return fmt.Errorf("target RELAYAI_MASTER_KEY: %w", err)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Minute)
	defer cancel()
	if os.Getenv("DATABASE_URL") == "" {
		return fmt.Errorf("target DATABASE_URL is required")
	}
	conn, err := pgx.Connect(ctx, os.Getenv("DATABASE_URL"))
	if err != nil {
		return fmt.Errorf("cannot connect to target database; check deployment configuration")
	}
	defer conn.Close(context.Background())
	report, err := legacyimport.Import(ctx, conn, p, key, *apply)
	if err != nil {
		return err
	}
	return json.NewEncoder(os.Stdout).Encode(report)
}
