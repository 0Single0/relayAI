package store

import (
	"context"
	"crypto/pbkdf2"
	"crypto/rand"
	"crypto/sha256"
	"crypto/subtle"
	"encoding/base64"
	"encoding/hex"
	"errors"
	"fmt"
	"strconv"
	"strings"
	"time"

	"database/sql"

	"github.com/jackc/pgx/v5/pgconn"
	"gorm.io/gorm"
)

// ---------------------------------------------------------------------------
// 密码哈希：PBKDF2-HMAC-SHA256
//
// 为什么不是 bcrypt：bcrypt 在 `golang.org/x/crypto` 里，那是外部模块。
// 本机装它时 Go 要解析整个依赖图并访问 proxy.golang.org，网络不通就装不上；
// 而 Go 1.24+ 的标准库已经有 `crypto/pbkdf2`。用标准库意味着
// **零外部依赖**，离线也能构建 —— 这对一个要能随时重建的服务很重要。
//
// 参数按 OWASP 2023 建议：PBKDF2-HMAC-SHA256 迭代 600,000 次。
// 迭代次数写进哈希串里（`pbkdf2-sha256$迭代$盐$哈希`），
// 将来提升强度时老密码仍然能验证，不需要强制所有人改密码。
// ---------------------------------------------------------------------------

const (
	// pbkdf2Iterations 是当前写入新密码时用的迭代次数。
	pbkdf2Iterations = 600_000
	// pbkdf2KeyLen 是派生密钥长度（32 字节 = 256 位，与 SHA-256 输出一致）。
	pbkdf2KeyLen = 32
	// pbkdf2SaltLen 是盐长度。
	pbkdf2SaltLen = 16
)

// hashPassword 生成 `pbkdf2-sha256$迭代$盐$哈希` 形式的字符串。
func hashPassword(password string) (string, error) {
	salt := make([]byte, pbkdf2SaltLen)
	if _, err := rand.Read(salt); err != nil {
		return "", fmt.Errorf("生成盐失败: %w", err)
	}

	key, err := pbkdf2.Key(sha256.New, password, salt, pbkdf2Iterations, pbkdf2KeyLen)
	if err != nil {
		return "", fmt.Errorf("密码派生失败: %w", err)
	}

	// 用 base64 RawURLEncoding：不含 `$`，不会破坏分隔符。
	encodedSalt := base64.RawURLEncoding.EncodeToString(salt)
	encodedKey := base64.RawURLEncoding.EncodeToString(key)

	return fmt.Sprintf("pbkdf2-sha256$%d$%s$%s", pbkdf2Iterations, encodedSalt, encodedKey), nil
}

// verifyPassword 校验密码。
//
// 用 `subtle.ConstantTimeCompare` 而不是 `==`：字符串比较会在第一个不同字节处
// 提前返回，攻击者能靠响应时间逐字节猜出哈希。
func verifyPassword(encoded, password string) bool {
	parts := strings.Split(encoded, "$")
	if len(parts) != 4 || parts[0] != "pbkdf2-sha256" {
		return false
	}

	iterations, err := strconv.Atoi(parts[1])
	if err != nil || iterations <= 0 {
		return false
	}

	salt, err := base64.RawURLEncoding.DecodeString(parts[2])
	if err != nil {
		return false
	}

	expected, err := base64.RawURLEncoding.DecodeString(parts[3])
	if err != nil {
		return false
	}

	actual, err := pbkdf2.Key(sha256.New, password, salt, iterations, len(expected))
	if err != nil {
		return false
	}

	return subtle.ConstantTimeCompare(actual, expected) == 1
}

// ErrEmailTaken 表示邮箱已被注册。
var ErrEmailTaken = errors.New("该邮箱已注册")

// ErrBadCredentials 表示邮箱或密码不对。
//
// **登录失败不区分"邮箱不存在"与"密码错"**：分开报会把接口变成
// 账号枚举器（试探出哪些邮箱注册过）。所以两种情况统一这个词。
var ErrBadCredentials = errors.New("邮箱或密码不正确")

// ErrInsufficientBalance 表示扣减会让余额变成负数。
//
// 独立成哨兵错误而不是用 `fmt.Errorf` 拼一句话：
// handler 要据此回 **400**（请求本身不合理）而不是 500（服务坏了）。
// 500 会让用户以为系统故障，实际是他填的钱不对。
var ErrInsufficientBalance = errors.New("余额不足：扣减后会小于 0")

// ErrCodeInvalid 表示兑换码不可用（不存在/已用/过期）。
var ErrCodeInvalid = errors.New("兑换码无效或已被使用")

// User 是管理台用户。
type User struct {
	ID            int64   `json:"id"`
	Email         string  `json:"email"`
	Nickname      string  `json:"nickname"`
	Balance       float64 `json:"balance"`
	TotalRecharged float64 `json:"totalRecharged"`
	Role          string  `json:"role"`
	Enabled       bool    `json:"enabled"`
	Source        string  `json:"source"`
	CreatedAt     string  `json:"createdAt"`
	LastLoginAt   *string `json:"lastLoginAt"`
}

// RegisterInput 是注册输入。
type RegisterInput struct {
	Email    string `json:"email"`
	Password string `json:"password"`
	Nickname string `json:"nickname"`
}

// NormalizeEmail 统一小写并去空白。
//
// 必须归一：`A@x.com` 与 `a@x.com` 在用户心里是同一个邮箱，
// 不归一就会出现两个账号、余额各一半。
func NormalizeEmail(email string) string {
	return strings.ToLower(strings.TrimSpace(email))
}

// ValidatePassword 检查密码强度。
//
// 8 位是底线：单租户自用也挡一下"123456"，因为余额是真钱。
func ValidatePassword(password string) error {
	if len(password) < 8 {
		return fmt.Errorf("密码至少 8 位")
	}
	if len(password) > 128 {
		// bcrypt 只取前 72 字节；超过就直接拒，避免"长密码看似更安全"的误解。
		return fmt.Errorf("密码最多 128 位")
	}
	return nil
}

// RegisterUser 注册一个用户。
func (s *Store) RegisterUser(ctx context.Context, input RegisterInput) (*User, error) {
	email := NormalizeEmail(input.Email)
	if email == "" {
		return nil, fmt.Errorf("邮箱不能为空")
	}
	if err := ValidatePassword(input.Password); err != nil {
		return nil, err
	}

	hash, err := hashPassword(input.Password)
	if err != nil {
		return nil, err
	}

	nickname := strings.TrimSpace(input.Nickname)
	if nickname == "" {
		// 默认用邮箱前缀：比"用户12345"友好，也不需要用户多填一个字段。
		if at := strings.Index(email, "@"); at > 0 {
			nickname = email[:at]
		} else {
			nickname = email
		}
	}

	// 第一个注册的人是 admin（开箱即用，不用手工改库）。
	role := "user"
	var count int
	if err := s.db.WithContext(ctx).Raw("SELECT count(*) FROM users").Row().Scan(&count); err == nil && count == 0 {
		role = "admin"
	}

	const query = `
		INSERT INTO users (email, password_hash, nickname, role)
		VALUES ($1, $2, $3, $4)
		RETURNING id, email, nickname, balance::float8, total_recharged::float8,
		          role, enabled, source, created_at, last_login_at`

	var user User
	var createdAt any
	var lastLogin any
	err = s.db.WithContext(ctx).Raw(query, email, hash, nickname, role).Row().Scan(
		&user.ID, &user.Email, &user.Nickname, &user.Balance, &user.TotalRecharged,
		&user.Role, &user.Enabled, &user.Source, &createdAt, &lastLogin,
	)
	if err != nil {
		var pgErr *pgconn.PgError
		if errors.As(err, &pgErr) && pgErr.Code == "23505" {
			return nil, ErrEmailTaken
		}
		return nil, fmt.Errorf("注册失败: %w", err)
	}

	user.CreatedAt = formatTime(createdAt)
	user.LastLoginAt = nullableTime(lastLogin)
	return &user, nil
}

// VerifyLogin 校验邮箱密码，成功返回用户。
func (s *Store) VerifyLogin(ctx context.Context, email, password string) (*User, error) {
	const query = `
		SELECT id, email, nickname, password_hash, balance::float8, total_recharged::float8,
		       role, enabled, source, created_at, last_login_at
		FROM users WHERE email = $1`

	var user User
	var hash string
	var createdAt any
	var lastLogin any
	err := s.db.WithContext(ctx).Raw(query, NormalizeEmail(email)).Row().Scan(
		&user.ID, &user.Email, &user.Nickname, &hash, &user.Balance, &user.TotalRecharged,
		&user.Role, &user.Enabled, &user.Source, &createdAt, &lastLogin,
	)
	if errors.Is(err, sql.ErrNoRows) {
		return nil, ErrBadCredentials
	}
	if err != nil {
		return nil, fmt.Errorf("查询用户失败: %w", err)
	}

	if !user.Enabled {
		return nil, fmt.Errorf("该账号已被停用")
	}

	if !verifyPassword(hash, password) {
		return nil, ErrBadCredentials
	}

	user.CreatedAt = formatTime(createdAt)
	user.LastLoginAt = nullableTime(lastLogin)

	// 记录最后登录时间。失败不影响登录本身。
	_, _ = s.exec(ctx, "UPDATE users SET last_login_at = now() WHERE id = $1", user.ID)

	return &user, nil
}

// GetUser 按 id 取用户。
func (s *Store) GetUser(ctx context.Context, id int64) (*User, error) {
	const query = `
		SELECT id, email, nickname, balance::float8, total_recharged::float8,
		       role, enabled, source, created_at, last_login_at
		FROM users WHERE id = $1`

	var user User
	var createdAt any
	var lastLogin any
	err := s.db.WithContext(ctx).Raw(query, id).Row().Scan(
		&user.ID, &user.Email, &user.Nickname, &user.Balance, &user.TotalRecharged,
		&user.Role, &user.Enabled, &user.Source, &createdAt, &lastLogin,
	)
	if errors.Is(err, sql.ErrNoRows) {
		return nil, ErrNotFound
	}
	if err != nil {
		return nil, fmt.Errorf("查询用户失败: %w", err)
	}

	user.CreatedAt = formatTime(createdAt)
	user.LastLoginAt = nullableTime(lastLogin)
	return &user, nil
}

// ListUsers 分页列出用户（管理台用）。
func (s *Store) ListUsers(ctx context.Context, page, pageSize int) ([]User, int, error) {
	if page < 1 {
		page = 1
	}
	if pageSize < 1 || pageSize > 200 {
		pageSize = 50
	}

	var total int
	if err := s.db.WithContext(ctx).Raw("SELECT count(*) FROM users").Row().Scan(&total); err != nil {
		return nil, 0, fmt.Errorf("统计用户失败: %w", err)
	}

	const query = `
		SELECT id, email, nickname, balance::float8, total_recharged::float8,
		       role, enabled, source, created_at, last_login_at
		FROM users ORDER BY created_at DESC LIMIT $1 OFFSET $2`

	rows, err := s.db.WithContext(ctx).Raw(query, pageSize, (page-1)*pageSize).Rows()
	if err != nil {
		return nil, 0, fmt.Errorf("查询用户失败: %w", err)
	}
	defer rows.Close()

	list := make([]User, 0, pageSize)
	for rows.Next() {
		var user User
		var createdAt any
		var lastLogin any
		if err := rows.Scan(
			&user.ID, &user.Email, &user.Nickname, &user.Balance, &user.TotalRecharged,
			&user.Role, &user.Enabled, &user.Source, &createdAt, &lastLogin,
		); err != nil {
			return nil, 0, fmt.Errorf("扫描用户失败: %w", err)
		}
		user.CreatedAt = formatTime(createdAt)
		user.LastLoginAt = nullableTime(lastLogin)
		list = append(list, user)
	}

	return list, total, rows.Err()
}

// RedeemCode 是兑换码。
type RedeemCode struct {
	ID        int64    `json:"id"`
	Code      string   `json:"code"`
	Amount    float64  `json:"amount"`
	Note      string   `json:"note"`
	ExpiresAt *string  `json:"expiresAt"`
	UsedBy    *int64   `json:"usedBy"`
	UsedAt    *string  `json:"usedAt"`
	CreatedAt string   `json:"createdAt"`
}

// 易混字符表：去掉 0/O/1/I/L，避免人工抄写出错。
const codeAlphabet = "23456789ABCDEFGHJKMNPQRSTUVWXYZ"

// GenerateCode 生成一个 16 位兑换码（4 组 × 4 位）。
func GenerateCode() (string, error) {
	const groups = 4
	const groupLen = 4
	buf := make([]byte, groups*groupLen)
	if _, err := rand.Read(buf); err != nil {
		return "", fmt.Errorf("生成兑换码失败: %w", err)
	}

	out := make([]string, 0, groups)
	for group := 0; group < groups; group++ {
		part := make([]byte, groupLen)
		for index := 0; index < groupLen; index++ {
			part[index] = codeAlphabet[int(buf[group*groupLen+index])%len(codeAlphabet)]
		}
		out = append(out, string(part))
	}
	return strings.Join(out, "-"), nil
}

// CreateRedeemCodes 批量生成兑换码。
func (s *Store) CreateRedeemCodes(
	ctx context.Context,
	amount float64,
	count int,
	note string,
	expiresAt *time.Time,
) ([]RedeemCode, error) {
	if amount <= 0 {
		return nil, fmt.Errorf("面额必须大于 0")
	}
	if count < 1 || count > 500 {
		return nil, fmt.Errorf("一次最多生成 500 个")
	}

	const query = `
		INSERT INTO redeem_codes (code, amount, note, expires_at)
		VALUES ($1, $2, $3, $4)
		RETURNING id, code, amount::float8, note, expires_at, used_by, used_at, created_at`

	created := make([]RedeemCode, 0, count)
	for index := 0; index < count; index++ {
		code, err := GenerateCode()
		if err != nil {
			return nil, err
		}

		var item RedeemCode
		var expires any
		var usedBy *int64
		var usedAt any
		var createdAt any
		if err := s.db.WithContext(ctx).Raw(query, code, amount, note, expiresAt).Row().Scan(
			&item.ID, &item.Code, &item.Amount, &item.Note,
			&expires, &usedBy, &usedAt, &createdAt,
		); err != nil {
			return nil, fmt.Errorf("生成兑换码失败: %w", err)
		}

		item.ExpiresAt = nullableTime(expires)
		item.UsedBy = usedBy
		item.UsedAt = nullableTime(usedAt)
		item.CreatedAt = formatTime(createdAt)
		created = append(created, item)
	}

	return created, nil
}

// ListRedeemCodes 列出兑换码。
func (s *Store) ListRedeemCodes(ctx context.Context, page, pageSize int) ([]RedeemCode, int, error) {
	if page < 1 {
		page = 1
	}
	if pageSize < 1 || pageSize > 200 {
		pageSize = 50
	}

	var total int
	if err := s.db.WithContext(ctx).Raw("SELECT count(*) FROM redeem_codes").Row().Scan(&total); err != nil {
		return nil, 0, fmt.Errorf("统计兑换码失败: %w", err)
	}

	const query = `
		SELECT id, code, amount::float8, note, expires_at, used_by, used_at, created_at
		FROM redeem_codes ORDER BY created_at DESC LIMIT $1 OFFSET $2`

	rows, err := s.db.WithContext(ctx).Raw(query, pageSize, (page-1)*pageSize).Rows()
	if err != nil {
		return nil, 0, fmt.Errorf("查询兑换码失败: %w", err)
	}
	defer rows.Close()

	list := make([]RedeemCode, 0, pageSize)
	for rows.Next() {
		var item RedeemCode
		var expires any
		var usedBy *int64
		var usedAt any
		var createdAt any
		if err := rows.Scan(
			&item.ID, &item.Code, &item.Amount, &item.Note,
			&expires, &usedBy, &usedAt, &createdAt,
		); err != nil {
			return nil, 0, fmt.Errorf("扫描兑换码失败: %w", err)
		}
		item.ExpiresAt = nullableTime(expires)
		item.UsedBy = usedBy
		item.UsedAt = nullableTime(usedAt)
		item.CreatedAt = formatTime(createdAt)
		list = append(list, item)
	}

	return list, total, rows.Err()
}

// RedeemForUser 核销兑换码并充值。
//
// **必须在一个事务里**：读码 → 标已用 → 加余额 → 记流水。
// 不放在事务里就会出现两个典型事故：
//   - 两个人同时兑换同一个码，都通过了"未使用"检查 → 白送一次；
//   - 余额加了但码没标已用（或反之）→ 对不上账。
//
// 用 `UPDATE ... WHERE used_at IS NULL RETURNING` 做**原子占位**：
// 只有真正把 used_at 从 NULL 改成时间的那个事务能拿到行，
// 另一个事务拿到 0 行，直接判"已被使用"。这样连行锁都不用显式加。
func (s *Store) RedeemForUser(ctx context.Context, userID int64, code string) (*User, float64, error) {
	normalized := strings.ToUpper(strings.TrimSpace(code))


	var codeID int64
	var amount float64
	var balance float64

	// 整段放进一个事务：核销、加余额、写流水必须同生共死。
	// 只核销不加钱（或反过来）会让账目永久对不上。
	txErr := s.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		//
		// 原子占位：只有把 used_at 从 NULL 改成时间的那个事务拿到行。
		// 并发重复兑换时另一个事务得到 0 行，直接判"已被使用"。
		redeem := tx.Raw(
			`UPDATE redeem_codes SET used_by = ?, used_at = now()
			  WHERE code = ? AND used_at IS NULL
			    AND (expires_at IS NULL OR expires_at > now())
			 RETURNING id, amount::float8`,
			userID, normalized,
		)
		var redeemed struct {
			ID     int64   `gorm:"column:id"`
			Amount float64 `gorm:"column:amount"`
		}
		if err := redeem.Row().Scan(&redeemed.ID, &redeemed.Amount); err != nil {
			if errors.Is(err, sql.ErrNoRows) {
				return ErrCodeInvalid
			}
			return fmt.Errorf("核销兑换码失败: %w", err)
		}
		codeID = redeemed.ID
		amount = redeemed.Amount

		credit := tx.Raw(
			`UPDATE users
			    SET balance = balance + ?,
			        total_recharged = total_recharged + ?
			  WHERE id = ?
			 RETURNING balance::float8`,
			amount, amount, userID,
		)
		if err := credit.Row().Scan(&balance); err != nil {
			return fmt.Errorf("充值失败: %w", err)
		}

		return tx.Exec(
			`INSERT INTO balance_transactions (user_id, amount, balance_after, kind, ref_id, note)
			 VALUES (?, ?, ?, 'redeem', ?, ?)`,
			userID, amount, balance, codeID, "兑换码充值",
		).Error
	})
	if txErr != nil {
		if errors.Is(txErr, ErrCodeInvalid) {
			return nil, 0, ErrCodeInvalid
		}
		return nil, 0, txErr
	}

	user, err := s.GetUser(ctx, userID)
	if err != nil {
		return nil, 0, err
	}
	return user, amount, nil
}

// BalanceTransaction 是一条余额流水。
type BalanceTransaction struct {
	ID           int64   `json:"id"`
	Amount       float64 `json:"amount"`
	BalanceAfter float64 `json:"balanceAfter"`
	Kind         string  `json:"kind"`
	Note         string  `json:"note"`
	CreatedAt    string  `json:"createdAt"`
}

// ListBalanceTransactions 列出某用户的余额流水。
func (s *Store) ListBalanceTransactions(ctx context.Context, userID int64, page, pageSize int) ([]BalanceTransaction, int, error) {
	if page < 1 {
		page = 1
	}
	if pageSize < 1 || pageSize > 200 {
		pageSize = 50
	}

	/*
		**只列充值类流水，不含逐次扣费。**

		为什么：扣费是"用一次扣一点"的高频记录，几条就能把充值记录挤出视野，
		而用户来这一页要看的是"我充了多少、还剩多少"。逐次消费的明细
		在「使用记录」页有完整账单（含单价与倍率），两边各司其职。

		口径：金额为正的入账（充值 / 兑换 / 管理员加款）。
		退款若为负数则不算充值，不会出现在这里。
	*/
	const where = "user_id = $1 AND amount > 0 AND kind <> 'usage'"

	var total int
	if err := s.db.WithContext(ctx).Raw("SELECT count(*) FROM balance_transactions WHERE "+where, userID,
	).Row().Scan(&total); err != nil {
		return nil, 0, fmt.Errorf("统计流水失败: %w", err)
	}

	const query = `
		SELECT id, amount::float8, balance_after::float8, kind, note, created_at
		FROM balance_transactions WHERE ` + where + `
		ORDER BY created_at DESC LIMIT $2 OFFSET $3`

	rows, err := s.db.WithContext(ctx).Raw(query, userID, pageSize, (page-1)*pageSize).Rows()
	if err != nil {
		return nil, 0, fmt.Errorf("查询流水失败: %w", err)
	}
	defer rows.Close()

	list := make([]BalanceTransaction, 0, pageSize)
	for rows.Next() {
		var item BalanceTransaction
		var createdAt any
		if err := rows.Scan(
			&item.ID, &item.Amount, &item.BalanceAfter, &item.Kind, &item.Note, &createdAt,
		); err != nil {
			return nil, 0, fmt.Errorf("扫描流水失败: %w", err)
		}
		item.CreatedAt = formatTime(createdAt)
		list = append(list, item)
	}

	return list, total, rows.Err()
}

// ---------------------------------------------------------------------------
// 会话令牌
//
// 为什么不用 JWT：单进程、单租户，一个能吊销的随机令牌比 JWT 简单得多
// （JWT 想吊销就得再引一张黑名单表，反而更重）。
// 令牌本身只存 SHA-256：库里被读走也拿不到可用的令牌。
// ---------------------------------------------------------------------------

// SessionTokenTTL 是令牌有效期。
const SessionTokenTTL = 7 * 24 * time.Hour

// CreateSession 为用户签发一个令牌，返回**明文**（只此一次）。
func (s *Store) CreateSession(ctx context.Context, userID int64) (string, error) {
	raw := make([]byte, 32)
	if _, err := rand.Read(raw); err != nil {
		return "", fmt.Errorf("生成令牌失败: %w", err)
	}
	token := base64.RawURLEncoding.EncodeToString(raw)

	digest := sha256.Sum256([]byte(token))
	hash := hex.EncodeToString(digest[:])

	_, err := s.exec(ctx,
		`INSERT INTO user_sessions (user_id, token_hash, expires_at)
		 VALUES ($1, $2, now() + make_interval(secs => $3))`,
		userID, hash, int(SessionTokenTTL.Seconds()),
	)
	if err != nil {
		return "", fmt.Errorf("保存会话失败: %w", err)
	}

	return token, nil
}

// UserByToken 按令牌取用户。
func (s *Store) UserByToken(ctx context.Context, token string) (*User, error) {
	if strings.TrimSpace(token) == "" {
		return nil, ErrNotFound
	}

	digest := sha256.Sum256([]byte(token))
	hash := hex.EncodeToString(digest[:])

	const query = `
		SELECT u.id, u.email, u.nickname, u.balance::float8, u.total_recharged::float8,
		       u.role, u.enabled, u.source, u.created_at, u.last_login_at
		FROM user_sessions s
		JOIN users u ON u.id = s.user_id
		WHERE s.token_hash = $1 AND s.expires_at > now() AND u.enabled`

	var user User
	var createdAt any
	var lastLogin any
	err := s.db.WithContext(ctx).Raw(query, hash).Row().Scan(
		&user.ID, &user.Email, &user.Nickname, &user.Balance, &user.TotalRecharged,
		&user.Role, &user.Enabled, &user.Source, &createdAt, &lastLogin,
	)
	if errors.Is(err, sql.ErrNoRows) {
		return nil, ErrNotFound
	}
	if err != nil {
		return nil, fmt.Errorf("查询会话失败: %w", err)
	}

	user.CreatedAt = formatTime(createdAt)
	user.LastLoginAt = nullableTime(lastLogin)
	return &user, nil
}

// DeleteSession 吊销令牌（登出）。
func (s *Store) DeleteSession(ctx context.Context, token string) error {
	digest := sha256.Sum256([]byte(token))
	hash := hex.EncodeToString(digest[:])
	_, err := s.exec(ctx, "DELETE FROM user_sessions WHERE token_hash = $1", hash)
	return err
}

// AdminUpdateUserInput 是管理员编辑用户的输入。
//
// 全部字段可空：界面只提交改动的字段，而不是整行覆盖 ——
// 整行覆盖会让"只想改昵称"的操作把余额一起写回去。
type AdminUpdateUserInput struct {
	Nickname *string `json:"nickname"`
	Role     *string `json:"role"`
	Enabled  *bool   `json:"enabled"`
}

// UpdateUserByAdmin 由管理员编辑用户资料。
func (s *Store) UpdateUserByAdmin(ctx context.Context, id int64, input AdminUpdateUserInput) (*User, error) {
	sets := make([]string, 0, 3)
	args := make([]any, 0, 4)
	arg := func(value any) string {
		args = append(args, value)
		return fmt.Sprintf("$%d", len(args))
	}

	if input.Nickname != nil {
		sets = append(sets, "nickname = "+arg(strings.TrimSpace(*input.Nickname)))
	}
	if input.Role != nil {
		role := *input.Role
		if role != "admin" && role != "user" {
			return nil, fmt.Errorf("角色只能是 admin 或 user")
		}
		sets = append(sets, "role = "+arg(role))
	}
	if input.Enabled != nil {
		sets = append(sets, "enabled = "+arg(*input.Enabled))
	}

	if len(sets) == 0 {
		return s.GetUser(ctx, id)
	}

	query := "UPDATE users SET " + strings.Join(sets, ", ") + " WHERE id = " + arg(id)
	affected, err := s.exec(ctx, query, args...)
	if err != nil {
		return nil, fmt.Errorf("更新用户失败: %w", err)
	}
	if affected == 0 {
		return nil, ErrNotFound
	}

	return s.GetUser(ctx, id)
}

// ResetPassword 由管理员重置某用户的密码。
//
// 重置后**必须吊销该用户所有会话**：否则旧令牌还能继续用，
// 而"重置密码"在用户心里等同于"把别人踢下线"。这两件事必须一起做。
func (s *Store) ResetPassword(ctx context.Context, id int64, password string) error {
	if err := ValidatePassword(password); err != nil {
		return err
	}

	hash, err := hashPassword(password)
	if err != nil {
		return err
	}

	// 改密码 + 吊销会话必须同时成功：
	// 只改密码不吊销，旧令牌还能继续用（改密码就失去了"踢下线"的意义）。
	return s.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		update := tx.Exec("UPDATE users SET password_hash = $2 WHERE id = $1", id, hash)
		if update.Error != nil {
			return fmt.Errorf("重置密码失败: %w", update.Error)
		}
		if update.RowsAffected == 0 {
			return ErrNotFound
		}

		if err := tx.Exec("DELETE FROM user_sessions WHERE user_id = $1", id).Error; err != nil {
			return fmt.Errorf("吊销会话失败: %w", err)
		}
		return nil
	})
}

// AdminCreateUserInput 是管理员创建用户的输入。
//
// 与自助注册的区别：管理员可以直接指定角色与初始余额，
// 而且**不需要**走"第一个用户才是 admin"的判定 —— 那是给自助注册用的。
type AdminCreateUserInput struct {
	Email    string  `json:"email"`
	Password string  `json:"password"`
	Nickname string  `json:"nickname"`
	Role     string  `json:"role"`
	Balance  float64 `json:"balance"`
}

// CreateUserByAdmin 由管理员创建用户。
func (s *Store) CreateUserByAdmin(ctx context.Context, input AdminCreateUserInput) (*User, error) {
	email := NormalizeEmail(input.Email)
	if email == "" {
		return nil, fmt.Errorf("邮箱不能为空")
	}
	if err := ValidatePassword(input.Password); err != nil {
		return nil, err
	}

	role := input.Role
	if role != "admin" && role != "user" {
		role = "user"
	}
	if input.Balance < 0 {
		return nil, fmt.Errorf("初始余额不能为负")
	}

	hash, err := hashPassword(input.Password)
	if err != nil {
		return nil, err
	}

	nickname := strings.TrimSpace(input.Nickname)
	if nickname == "" {
		if at := strings.Index(email, "@"); at > 0 {
			nickname = email[:at]
		} else {
			nickname = email
		}
	}

	const query = `
		INSERT INTO users (email, password_hash, nickname, role, balance, total_recharged, source)
		VALUES ($1, $2, $3, $4, $5, $5, 'manual')
		RETURNING id, email, nickname, balance::float8, total_recharged::float8,
		          role, enabled, source, created_at, last_login_at`

	var user User
	var createdAt any
	var lastLogin any

	// 建用户 + 记初始余额流水在一个事务里：
	// 只建用户不记流水会出现"余额 100 但查不到这钱哪来的"。
	txErr := s.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		if err := tx.Raw(query, email, hash, nickname, role, input.Balance).Row().Scan(
			&user.ID, &user.Email, &user.Nickname, &user.Balance, &user.TotalRecharged,
			&user.Role, &user.Enabled, &user.Source, &createdAt, &lastLogin,
		); err != nil {
			var pgErr *pgconn.PgError
			if errors.As(err, &pgErr) && pgErr.Code == "23505" {
				return ErrEmailTaken
			}
			return fmt.Errorf("创建用户失败: %w", err)
		}

		if input.Balance > 0 {
			if err := tx.Exec(
				`INSERT INTO balance_transactions (user_id, amount, balance_after, kind, note)
				 VALUES ($1, $2, $2, 'adjust', '管理员开户初始余额')`,
				user.ID, input.Balance,
			).Error; err != nil {
				return fmt.Errorf("记录初始余额失败: %w", err)
			}
		}
		return nil
	})
	if txErr != nil {
		if errors.Is(txErr, ErrEmailTaken) {
			return nil, ErrEmailTaken
		}
		return nil, txErr
	}

	user.CreatedAt = formatTime(createdAt)
	user.LastLoginAt = nullableTime(lastLogin)
	return &user, nil
}

// AdjustBalance 管理员调整用户余额（正数加、负数减）。
//
// 减到负数直接拒绝：允许负余额意味着"用户可以透支跑请求"，
// 而额度检查会在余额 0 时就把请求挡掉，透支永远收不回来。
func (s *Store) AdjustBalance(ctx context.Context, userID int64, delta float64, note string) (*User, error) {
	if delta == 0 {
		return nil, fmt.Errorf("调整金额不能为 0")
	}

	// 条件更新保证原子性：`balance + delta >= 0` 在 SQL 里判断，
	// 而不是先查再改 —— 先查再改在并发下会漏判。
	var balance float64

	txErr := s.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
		if err := tx.Raw(
			`UPDATE users
			    SET balance = balance + $2,
			        total_recharged = total_recharged + GREATEST($2, 0)
			  WHERE id = $1 AND balance + $2 >= 0
			 RETURNING balance::float8`,
			userID, delta,
		).Row().Scan(&balance); err != nil {
			if errors.Is(err, sql.ErrNoRows) {
				// 两种可能：用户不存在，或扣到负数。分别报清楚。
				var exists bool
				probe := tx.Raw("SELECT true FROM users WHERE id = $1", userID).Row().Scan(&exists)
				if probe != nil {
					return ErrNotFound
				}
				return ErrInsufficientBalance
			}
			return fmt.Errorf("调整余额失败: %w", err)
		}

		return tx.Exec(
			`INSERT INTO balance_transactions (user_id, amount, balance_after, kind, note)
			 VALUES ($1, $2, $3, 'adjust', $4)`,
			userID, delta, balance, note,
		).Error
	})
	if txErr != nil {
		if errors.Is(txErr, ErrNotFound) || errors.Is(txErr, ErrInsufficientBalance) {
			return nil, txErr
		}
		return nil, txErr
	}

	return s.GetUser(ctx, userID)
}
