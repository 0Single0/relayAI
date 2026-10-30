package gateway

import (
	"context"
	"fmt"
	"net/http"
	"net/http/httptest"
	"testing"

	"relayai/backend/internal/config"
)

func TestTaskGrowthPreservesPartialFailureAndZero(t *testing.T) {
	requests := 0
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		requests++
		fmt.Fprint(w, `{"channelId":"wb-cn","accounts":[{"uid":"first","growth":{"energy":null,"energyError":"HTTP 401: authorization required","streakDays":null,"streakError":"missing streak"}},{"uid":"second","growth":{"energy":0,"streakDays":15}}]}`)
	}))
	defer server.Close()
	client := New(config.GatewayConf{BaseURL: server.URL})
	for range 2 {
		live, err := client.GetTaskLive(context.Background(), "wb-cn")
		if err != nil {
			t.Fatal(err)
		}
		first := live.Accounts[0].Growth
		if first.Energy != nil || first.EnergyError == nil || *first.EnergyError != "HTTP 401: authorization required" || first.StreakError == nil {
			t.Fatalf("lost growth diagnostic: %+v", first)
		}
		second := live.Accounts[1].Growth
		if second.Energy == nil || *second.Energy != 0 || second.StreakDays == nil || *second.StreakDays != 15 {
			t.Fatalf("zero must remain a real value: %+v", second)
		}
	}
	if requests != 2 {
		t.Fatalf("expected a fresh gateway query each time, got %d", requests)
	}
}
