package store

import (
	"context"
	"errors"
	"fmt"
	"strings"

	"github.com/jackc/pgx/v5/pgconn"
)

// Proxy 是一条出站代理的管理面视图。
//
// **永不包含密码明文**（与账号凭据同一条硬规则）。
type Proxy struct {
	ID       int64  `json:"id"`
	Name     string `json:"name"`
	Protocol string `json:"protocol"`
	Host     string `json:"host"`
	Port     int    `json:"port"`
	Username string `json:"username"`
	// HasPassword 只表示"设过密码"，不返回值本身。
	HasPassword bool `json:"hasPassword"`
	// Fallback 是代理不可用时的策略：`fail` / `direct`。
	Fallback string `json:"fallback"`
	Enabled  bool   `json:"enabled"`
	Note     string `json:"note"`
	// AccountCount 是绑定到该代理的账号数（删除前能看出影响面）。
	AccountCount int    `json:"accountCount"`
	CreatedAt    string `json:"createdAt"`
	UpdatedAt    string `json:"updatedAt"`
}

// ProxyInput 是新建/编辑代理的输入。
type ProxyInput struct {
	Name     string `json:"name"`
	Protocol string `json:"protocol"`
	Host     string `json:"host"`
	Port     int    `json:"port"`
	Username string `json:"username"`
	// Password 为 nil 表示"不改密码"（编辑时不填就保留原值）。
	Password *string `json:"password"`
	// PasswordEnc 是网关加密后的密文，由 handler 填，不接受外部输入。
	PasswordEnc *string `json:"-"`
	Fallback    string  `json:"fallback"`
	Enabled     *bool   `json:"enabled"`
	Note        string  `json:"note"`
}

// ErrProxyEndpointTaken 表示同协议 + 同 host:port 已存在。
var ErrProxyEndpointTaken = errors.New("该代理地址已存在")

// ListProxies 返回全部代理（含绑定账号数）。
func (s *Store) ListProxies(ctx context.Context) ([]Proxy, error) {
	const query = `
		SELECT p.id, p.name, p.protocol, p.host, p.port, p.username,
		       p.password_enc <> '' AS has_password,
		       p.fallback, p.enabled, p.note,
		       COALESCE(count(a.id), 0) AS account_count,
		       p.created_at, p.updated_at
		FROM proxies p
		LEFT JOIN accounts a ON a.proxy_id = p.id
		GROUP BY p.id
		ORDER BY p.name`

	rows, err := s.pool.Query(ctx, query)
	if err != nil {
		return nil, fmt.Errorf("查询代理失败: %w", err)
	}
	defer rows.Close()

	list := make([]Proxy, 0, 8)
	for rows.Next() {
		var item Proxy
		var createdAt, updatedAt any
		if err := rows.Scan(
			&item.ID, &item.Name, &item.Protocol, &item.Host, &item.Port, &item.Username,
			&item.HasPassword, &item.Fallback, &item.Enabled, &item.Note,
			&item.AccountCount, &createdAt, &updatedAt,
		); err != nil {
			return nil, fmt.Errorf("扫描代理失败: %w", err)
		}
		item.CreatedAt = formatTime(createdAt)
		item.UpdatedAt = formatTime(updatedAt)
		list = append(list, item)
	}

	return list, rows.Err()
}

// CreateProxy 新建代理。
func (s *Store) CreateProxy(ctx context.Context, input ProxyInput) (*Proxy, error) {
	if err := validateProxy(input); err != nil {
		return nil, err
	}

	enabled := true
	if input.Enabled != nil {
		enabled = *input.Enabled
	}

	password := ""
	if input.PasswordEnc != nil {
		password = *input.PasswordEnc
	}

	const query = `
		INSERT INTO proxies (name, protocol, host, port, username, password_enc, fallback, enabled, note)
		VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
		RETURNING id`

	var id int64
	err := s.pool.QueryRow(ctx, query,
		input.Name, input.Protocol, input.Host, input.Port,
		input.Username, password, fallbackOr(input.Fallback), enabled, input.Note,
	).Scan(&id)
	if err != nil {
		// `proxies_by_endpoint` 唯一索引冲突：翻译成人话，而不是把约束名抛给用户。
		var pgErr *pgconn.PgError
		if errors.As(err, &pgErr) && pgErr.Code == "23505" {
			return nil, ErrProxyEndpointTaken
		}
		return nil, fmt.Errorf("新建代理失败: %w", err)
	}

	return s.GetProxy(ctx, id)
}

// UpdateProxy 编辑代理。
func (s *Store) UpdateProxy(ctx context.Context, id int64, input ProxyInput) (*Proxy, error) {
	if err := validateProxy(input); err != nil {
		return nil, err
	}

	const query = `
		UPDATE proxies SET
		    name = $2,
		    protocol = $3,
		    host = $4,
		    port = $5,
		    username = $6,
		    -- NULL 表示"不改密码"：编辑表单里密码默认是空的，
		    -- 若一律覆盖，用户只想改名字也会把密码清空。
		    password_enc = COALESCE($7, password_enc),
		    fallback = $8,
		    enabled = COALESCE($9, enabled),
		    note = $10,
		    updated_at = now()
		WHERE id = $1`

	result, err := s.pool.Exec(ctx, query,
		id, input.Name, input.Protocol, input.Host, input.Port,
		input.Username, input.PasswordEnc, fallbackOr(input.Fallback),
		input.Enabled, input.Note,
	)
	if err != nil {
		var pgErr *pgconn.PgError
		if errors.As(err, &pgErr) && pgErr.Code == "23505" {
			return nil, ErrProxyEndpointTaken
		}
		return nil, fmt.Errorf("更新代理失败: %w", err)
	}
	if result.RowsAffected() == 0 {
		return nil, ErrNotFound
	}

	return s.GetProxy(ctx, id)
}

// DeleteProxy 删除代理。绑定它的账号会被置空（走直连），不级联删账号。
func (s *Store) DeleteProxy(ctx context.Context, id int64) error {
	result, err := s.pool.Exec(ctx, "DELETE FROM proxies WHERE id = $1", id)
	if err != nil {
		return fmt.Errorf("删除代理失败: %w", err)
	}
	if result.RowsAffected() == 0 {
		return ErrNotFound
	}
	return nil
}

// GetProxy 按 id 取一条。
func (s *Store) GetProxy(ctx context.Context, id int64) (*Proxy, error) {
	list, err := s.ListProxies(ctx)
	if err != nil {
		return nil, err
	}
	for _, item := range list {
		if item.ID == id {
			return &item, nil
		}
	}
	return nil, ErrNotFound
}

// SetAccountProxy 给账号绑定/解绑代理（proxyID 为 nil 表示解绑）。
func (s *Store) SetAccountProxy(ctx context.Context, accountID int64, proxyID *int64) error {
	result, err := s.pool.Exec(ctx, "UPDATE accounts SET proxy_id = $2 WHERE id = $1", accountID, proxyID)
	if err != nil {
		return fmt.Errorf("绑定代理失败: %w", err)
	}
	if result.RowsAffected() == 0 {
		return ErrNotFound
	}
	return nil
}

func validateProxy(input ProxyInput) error {
	if strings.TrimSpace(input.Name) == "" {
		return fmt.Errorf("代理名称不能为空")
	}
	if strings.TrimSpace(input.Host) == "" {
		return fmt.Errorf("代理地址不能为空")
	}
	switch input.Protocol {
	case "socks5", "http", "https":
	default:
		// reqwest 只认这三种；写别的会在网关侧构建失败，
		// 不如在这里就拒掉，错误信息离用户更近。
		return fmt.Errorf("代理协议只支持 socks5 / http / https")
	}
	if input.Port <= 0 || input.Port > 65535 {
		return fmt.Errorf("端口需在 1–65535 之间")
	}
	if input.Fallback != "" && input.Fallback != "fail" && input.Fallback != "direct" {
		return fmt.Errorf("回退策略只能是 fail 或 direct")
	}
	return nil
}

func fallbackOr(value string) string {
	if value == "" {
		// 默认 fail：**不静默直连**。用户绑代理通常就是为了隐藏真实 IP，
		// 代理挂了直接走直连会把它暴露出去。
		return "fail"
	}
	return value
}
