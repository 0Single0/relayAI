package legacyimport

import (
	"bytes"
	"context"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"os"
	"testing"
	"time"

	"github.com/jackc/pgx/v5"
)

func fixture(t *testing.T) (Bundle, []byte, []byte) {
	t.Helper()
	sourceKey, targetKey := bytes.Repeat([]byte{7}, 32), bytes.Repeat([]byte{8}, 32)
	plain := []byte(`{"access_token":"fixture-access","refresh_token":"fixture-refresh","uid":"fixture-uid","domain":"example.invalid"}`)
	sealed, err := seal(sourceKey, plain)
	if err != nil {
		t.Fatal(err)
	}
	return Bundle{Version: 1, Channels: []Channel{{ID: "fixture-cn", Name: "Fixture CN", Adapter: "workbuddy", Enabled: true, Priority: 100, Weight: 1, Options: json.RawMessage(`{"realm":"cn","outbound_identity":"workbuddy"}`)}},
		Accounts: []Account{{ChannelID: "fixture-cn", UID: "fixture-uid", Nickname: "测试账号", Enabled: 1, Credential: string(sealed), AddedAt: 1700000000000, Source: "oauth"}}}, sourceKey, targetKey
}

func TestCredentialMigrationAndTampering(t *testing.T) {
	b, sourceKey, targetKey := fixture(t)
	p, err := prepare(b, sourceKey)
	if err != nil {
		t.Fatal(err)
	}
	sealed, err := seal(targetKey, p.accounts[0].Plaintext)
	if err != nil {
		t.Fatal(err)
	}
	plain, err := open(targetKey, string(sealed))
	if err != nil || !bytes.Equal(plain, p.accounts[0].Plaintext) {
		t.Fatal("credential payload changed")
	}
	if _, err = open(sourceKey, string(sealed)); err == nil {
		t.Fatal("source key still decrypts target envelope")
	}
	raw, _ := base64.StdEncoding.DecodeString(string(sealed))
	raw[len(raw)-1] ^= 1
	if _, err = open(targetKey, base64.StdEncoding.EncodeToString(raw)); err == nil {
		t.Fatal("tampering was accepted")
	}
	if _, err = prepare(b, targetKey); err == nil {
		t.Fatal("wrong source key was accepted")
	}
	b.Accounts = append(b.Accounts, b.Accounts[0])
	if _, err = prepare(b, sourceKey); err == nil {
		t.Fatal("duplicate uid was accepted")
	}
}

func TestImportTransactionAndIdempotency(t *testing.T) {
	dsn := os.Getenv("TEST_DATABASE_URL_LEGACY")
	if dsn == "" {
		t.Skip("set TEST_DATABASE_URL_LEGACY for isolated PostgreSQL schema test")
	}
	ctx := context.Background()
	conn, err := pgx.Connect(ctx, dsn)
	if err != nil {
		t.Fatal("cannot connect to integration test database")
	}
	defer conn.Close(ctx)
	schema := fmt.Sprintf("legacy_import_test_%d", time.Now().UnixNano())
	if _, err = conn.Exec(ctx, "CREATE SCHEMA "+schema); err != nil {
		t.Fatal(err)
	}
	defer conn.Exec(ctx, "DROP SCHEMA "+schema+" CASCADE")
	if _, err = conn.Exec(ctx, "SET search_path TO "+schema); err != nil {
		t.Fatal(err)
	}
	_, err = conn.Exec(ctx, `CREATE TABLE channels (id text PRIMARY KEY,name text,adapter text,enabled boolean,priority integer,weight integer,options jsonb);
	CREATE TABLE accounts (uid text UNIQUE,channel_id text REFERENCES channels(id),nickname text,enabled boolean,credential_cipher bytea,
	priority_override integer,weight_override integer,max_inflight integer,model_rename jsonb,source text,added_at timestamptz,
	platform text,options jsonb,base_url text,protocol text);
	CREATE TABLE channel_caps (channel_id text PRIMARY KEY REFERENCES channels(id),tasks boolean,daily boolean,checkin boolean,updated_at timestamptz DEFAULT now())`)
	if err != nil {
		t.Fatal(err)
	}
	b, sourceKey, targetKey := fixture(t)
	p, err := prepare(b, sourceKey)
	if err != nil {
		t.Fatal(err)
	}
	r, err := Import(ctx, conn, p, targetKey, false)
	if err != nil || r.NewAccounts != 1 || r.NewChannels != 1 || r.NewCapabilityRecords != 1 {
		t.Fatalf("preview: %+v %v", r, err)
	}
	var count int
	if err = conn.QueryRow(ctx, "SELECT count(*) FROM channels").Scan(&count); err != nil || count != 0 {
		t.Fatal("preview wrote data")
	}
	r, err = Import(ctx, conn, p, targetKey, true)
	if err != nil || r.NewAccounts != 1 || r.NewCapabilityRecords != 1 {
		t.Fatalf("apply: %+v %v", r, err)
	}
	var tasks, daily, checkin bool
	if err = conn.QueryRow(ctx, "SELECT tasks,daily,checkin FROM channel_caps WHERE channel_id='fixture-cn'").Scan(&tasks, &daily, &checkin); err != nil || !tasks || !daily || !checkin {
		t.Fatal("imported domestic channel is missing task capabilities")
	}
	if err = conn.QueryRow(ctx, `SELECT count(*) FROM channels c JOIN channel_caps cap ON cap.channel_id=c.id
		WHERE c.enabled AND (cap.tasks OR cap.daily OR cap.checkin) AND EXISTS(SELECT 1 FROM accounts a WHERE a.channel_id=c.id AND a.enabled)`).Scan(&count); err != nil || count != 1 {
		t.Fatal("imported channel is absent from task discovery")
	}
	var sealed []byte
	var platform, realm, nickname string
	if err = conn.QueryRow(ctx, "SELECT credential_cipher,platform,options->>'realm',nickname FROM accounts").Scan(&sealed, &platform, &realm, &nickname); err != nil {
		t.Fatal(err)
	}
	plain, err := open(targetKey, string(sealed))
	if err != nil || !bytes.Equal(plain, p.accounts[0].Plaintext) || platform != "workbuddy" || realm != "cn" || nickname != "测试账号" {
		t.Fatal("stored account does not match source")
	}
	r, err = Import(ctx, conn, p, targetKey, true)
	if err != nil || r.NewAccounts != 0 || r.SkippedExistingAccounts != 1 || r.NewCapabilityRecords != 0 {
		t.Fatalf("repeat: %+v %v", r, err)
	}
	var unchanged []byte
	_ = conn.QueryRow(ctx, "SELECT credential_cipher FROM accounts").Scan(&unchanged)
	if !bytes.Equal(sealed, unchanged) {
		t.Fatal("existing credential was overwritten")
	}
	// Older imports already have accounts but no capability row. Rerun must repair it.
	if _, err = conn.Exec(ctx, "DELETE FROM channel_caps"); err != nil {
		t.Fatal(err)
	}
	r, err = Import(ctx, conn, p, targetKey, false)
	if err != nil || r.NewCapabilityRecords != 1 || r.NewAccounts != 0 {
		t.Fatalf("repair preview: %+v %v", r, err)
	}
	if err = conn.QueryRow(ctx, "SELECT count(*) FROM channel_caps").Scan(&count); err != nil || count != 0 {
		t.Fatal("repair preview wrote data")
	}
	r, err = Import(ctx, conn, p, targetKey, true)
	if err != nil || r.NewCapabilityRecords != 1 || r.SkippedExistingAccounts != 1 {
		t.Fatalf("repair: %+v %v", r, err)
	}
	if _, err = conn.Exec(ctx, "UPDATE channel_caps SET tasks=false"); err != nil {
		t.Fatal(err)
	}
	if _, err = Import(ctx, conn, p, targetKey, true); err != nil {
		t.Fatal(err)
	}
	if err = conn.QueryRow(ctx, "SELECT tasks FROM channel_caps").Scan(&tasks); err != nil || tasks {
		t.Fatal("existing capability settings were overwritten")
	}
	if _, err = Import(ctx, conn, p, sourceKey, true); err == nil {
		t.Fatal("incorrect target key was accepted")
	}
	intlBundle, _, _ := fixture(t)
	intlBundle.Channels[0].ID = "fixture-intl"
	intlBundle.Channels[0].Options = json.RawMessage(`{"realm":"intl","outbound_identity":"workbuddy"}`)
	intlBundle.Accounts[0].ChannelID = "fixture-intl"
	intl, err := prepare(intlBundle, sourceKey)
	if err != nil {
		t.Fatal(err)
	}
	if _, err = Import(ctx, conn, intl, targetKey, true); err != nil {
		t.Fatal(err)
	}
	if err = conn.QueryRow(ctx, "SELECT tasks,daily,checkin FROM channel_caps WHERE channel_id='fixture-intl'").Scan(&tasks, &daily, &checkin); err != nil || tasks || !daily || !checkin {
		t.Fatal("international channel capabilities do not match the adapter")
	}
	// Exercise the standalone server repair against imported groups with real account counts.
	_, err = conn.Exec(ctx, `INSERT INTO channels (id,name,adapter,enabled,priority,weight,options) VALUES
		('wb-cn','CN','workbuddy',true,100,1,'{"realm":"cn"}'),
		('wb-intl','Intl','workbuddy',true,100,1,'{"realm":"intl"}');
		INSERT INTO accounts (uid,channel_id,platform,enabled) VALUES ('repair-cn','wb-cn','workbuddy',true),('repair-intl','wb-intl','workbuddy',true)`)
	if err != nil {
		t.Fatal(err)
	}
	repairSQL, err := os.ReadFile("../../../scripts/repair-legacy-channel-caps.sql")
	if err != nil {
		t.Fatal(err)
	}
	for range 2 {
		if _, err = conn.Exec(ctx, string(repairSQL)); err != nil {
			t.Fatal(err)
		}
	}
	if err = conn.QueryRow(ctx, "SELECT count(*) FROM channel_caps WHERE channel_id IN ('wb-cn','wb-intl') AND daily AND checkin AND tasks=(channel_id='wb-cn')").Scan(&count); err != nil || count != 2 {
		t.Fatal("standalone repair failed or created duplicate capability records")
	}
	// Failure after creating another channel must roll back the entire transaction.
	_, err = conn.Exec(ctx, "ALTER TABLE accounts ADD CONSTRAINT test_reject CHECK (uid <> 'rejected-uid')")
	if err != nil {
		t.Fatal(err)
	}
	second := *p
	second.Channels = append([]Channel(nil), p.Channels...)
	second.Channels[0].ID = "second-channel"
	second.accounts = append([]preparedAccount(nil), p.accounts...)
	second.accounts[0].UID = "rejected-uid"
	second.accounts[0].ChannelID = "second-channel"
	if _, err = Import(ctx, conn, &second, targetKey, true); err == nil {
		t.Fatal("expected constraint failure")
	}
	_ = conn.QueryRow(ctx, "SELECT count(*) FROM channels WHERE id='second-channel'").Scan(&count)
	if count != 0 {
		t.Fatal("failed import left a channel behind")
	}
	if err = conn.QueryRow(ctx, "SELECT count(*) FROM channel_caps WHERE channel_id='second-channel'").Scan(&count); err != nil || count != 0 {
		t.Fatal("failed import left capability records behind")
	}
	_, err = conn.Exec(ctx, `UPDATE channels SET options='{"realm":"intl","outbound_identity":"workbuddy"}'::jsonb`)
	if err != nil {
		t.Fatal(err)
	}
	if _, err = Import(ctx, conn, p, targetKey, true); err == nil {
		t.Fatal("region conflict was ignored")
	}
}
