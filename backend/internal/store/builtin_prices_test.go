package store

import (
	"context"
	"fmt"
	"math"
	"net/url"
	"os"
	"strings"
	"testing"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/stdlib"
	"gorm.io/driver/postgres"
	"gorm.io/gorm"
	"gorm.io/gorm/logger"
)

func TestBuiltinPriceOfficialQuotes(t *testing.T) {
	for _, test := range []struct {
		name     string
		currency string
		want     ModelPrice
	}{
		{"hy3", "CNY", ModelPrice{1, 4, 0.25}},
		{"hy4-preview", "CNY", ModelPrice{6, 18, 0.3}},
		{"glm-5.3", "CNY", ModelPrice{8, 28, 2}},
		{"glm-5.3-flash", "CNY", ModelPrice{0.8, 2.8, 0.23}},
		{"glm-5.1", "CNY", ModelPrice{6, 24, 1.3}},
		{"kimi-k2.6", "CNY", ModelPrice{6.5, 27, 1.1}},
		{"minimax-m3", "USD", ModelPrice{0.3, 1.2, 0.06}},
		{"deepseek-v4.1-flash", "USD", ModelPrice{0.3, 1.2, 0.006}},
		{"deepseek-v4-pro", "USD", ModelPrice{1.32, 3.96, 0.044}},
		{"gemini-2.5-pro", "USD", ModelPrice{1.25, 10, 0.125}},
		{"gemini-2.5-flash", "USD", ModelPrice{0.3, 2.5, 0.03}},
		{"gpt-4o", "USD", ModelPrice{2.5, 10, 1.25}},
		{"gpt-4o-2024-05-13", "USD", ModelPrice{5, 15, 0}},
	} {
		t.Run(test.name, func(t *testing.T) {
			got, ok := lookupBuiltinPrice(test.name)
			if !ok || got.Currency != test.currency || got.ModelPrice != test.want {
				t.Fatalf("quote = %+v, found = %v; want %s %+v", got, ok, test.currency, test.want)
			}
		})
	}
}

func TestBuiltinPriceDoesNotGuessVariants(t *testing.T) {
	for _, name := range []string{"", "gpt-5.9", "gpt-5-unpublished", "glm-5.3-flash-unknown", "claude-opus-4-99", "hy4-preview-f", "kimi-k3-1", "kimi-k2.7", "deepseek-v3-1-volc", "gpt-4o-2099-01-01"} {
		if _, ok := lookupBuiltinPrice(name); ok {
			t.Errorf("unverified model %q must not inherit a prefix's price", name)
		}
	}
	if got, ok := lookupBuiltinPrice(" GLM-5.3 "); !ok || got.Input != 8 {
		t.Fatal("case and surrounding spaces should not prevent exact matching")
	}
}

func TestBuiltinPriceCurrenciesAndSources(t *testing.T) {
	for name, price := range BuiltinPrices {
		if name != strings.ToLower(strings.TrimSpace(name)) || price.Basis == "" {
			t.Errorf("%s: missing normalized ID or pricing basis", name)
		}
		u, err := url.Parse(price.SourceURL)
		if err != nil || u.Scheme != "https" || u.Host == "" {
			t.Errorf("%s: missing official source URL", name)
		}
		if price.Currency != "USD" && price.Currency != "CNY" {
			t.Errorf("%s: unsupported quote currency %s", name, price.Currency)
		}
		for _, value := range []float64{price.Input, price.Output, price.CacheRead} {
			if value < 0 || math.IsNaN(value) || math.IsInf(value, 0) {
				t.Errorf("%s: invalid quoted price %v", name, value)
			}
		}
		if price.Input == 0 && price.Output == 0 {
			t.Errorf("%s: all-zero/free quotes need explicit priced-state support", name)
		}
		got := price.inCNY(7.5)
		factor := 1.0
		if price.Currency == "USD" {
			factor = 7.5
		}
		if got != (ModelPrice{price.Input * factor, price.Output * factor, price.CacheRead * factor}) {
			t.Errorf("%s: currency conversion = %+v", name, got)
		}
	}
}

// This test creates and drops its own schema, never the application's public
// schema. It exercises PostgreSQL filtering, precision and repeat application.
func TestBuiltinPriceFillPreservesPartialManualPrices(t *testing.T) {
	dsn := os.Getenv("TEST_DATABASE_URL_PRICING")
	if dsn == "" {
		t.Skip("set TEST_DATABASE_URL_PRICING for isolated PostgreSQL schema test")
	}
	cfg, err := pgx.ParseConfig(dsn)
	if err != nil {
		t.Fatal("invalid pricing test database configuration")
	}
	schema := fmt.Sprintf("builtin_price_test_%d", time.Now().UnixNano())
	cfg.RuntimeParams["search_path"] = schema
	sqlDB := stdlib.OpenDB(*cfg)
	defer sqlDB.Close()
	db, err := gorm.Open(postgres.New(postgres.Config{Conn: sqlDB}), &gorm.Config{Logger: logger.Default.LogMode(logger.Silent)})
	if err != nil {
		t.Fatal("cannot connect to pricing test database")
	}
	if err = db.Exec("CREATE SCHEMA " + schema).Error; err != nil {
		t.Fatal(err)
	}
	defer db.Exec("DROP SCHEMA " + schema + " CASCADE")
	if err = db.Exec(`CREATE TABLE models (canonical_name text PRIMARY KEY,
		input_price numeric(14,8) NOT NULL DEFAULT 0, output_price numeric(14,8) NOT NULL DEFAULT 0,
		cache_read_price numeric(14,8) NOT NULL DEFAULT 0)`).Error; err != nil {
		t.Fatal(err)
	}
	if err = db.Exec(`INSERT INTO models (canonical_name,input_price,output_price,cache_read_price) VALUES
		('glm-5.3',0,0,0), ('gpt-5-mini',0,0,0), ('gpt-5',0,9,0),
		('glm-5.2',7,0,0), ('gemini-2.5-pro',0,0,0.1), ('hy3',3,4,0.2),
		('glm-5.3-unpublished',0,0,0)`).Error; err != nil {
		t.Fatal(err)
	}
	s := New(db)
	filled, err := s.ApplyBuiltinPrices(context.Background(), 7.5)
	if err != nil || filled != 2 {
		t.Fatalf("fill count = %d, error = %v; want 2", filled, err)
	}
	for name, want := range map[string]ModelPrice{
		"glm-5.3":             {8, 28, 2},
		"gpt-5-mini":          {1.875, 15, 0.1875},
		"gpt-5":               {0, 9, 0},
		"glm-5.2":             {7, 0, 0},
		"gemini-2.5-pro":      {0, 0, 0.1},
		"hy3":                 {3, 4, 0.2},
		"glm-5.3-unpublished": {0, 0, 0},
	} {
		var got struct{ Input, Output, CacheRead float64 }
		if err := db.Raw(`SELECT input_price::float8 AS input, output_price::float8 AS output,
			cache_read_price::float8 AS cache_read FROM models WHERE canonical_name=?`, name).Scan(&got).Error; err != nil {
			t.Fatal(err)
		}
		if got.Input != want.Input || got.Output != want.Output || got.CacheRead != want.CacheRead {
			t.Errorf("%s: persisted %+v, want %+v", name, got, want)
		}
	}
	filled, err = s.ApplyBuiltinPrices(context.Background(), 9)
	if err != nil || filled != 0 {
		t.Fatalf("repeat fill = %d, error = %v; want no changes", filled, err)
	}
}
