import { useCallback, useEffect, useMemo, useState } from "react";
import {
  KeyRound,
  PencilLine,
  Plus,
  RefreshCw,
  Search,
  UserPlus,
  Users,
  Wallet,
} from "lucide-react";
import {
  adjustBalance,
  createUser,
  listUsers,
  resetPassword,
  updateUser,
  type User,
} from "@/api/auth";
import { DataTable, type Column } from "@/components/common/DataTable";
import { EmptyState } from "@/components/common/EmptyState";
import { ErrorState } from "@/components/common/ErrorState";
import { MyBox } from "@/components/common/MyBox";
import { Pagination } from "@/components/common/Pagination";
import { RowActions } from "@/components/common/RowActions";
import { StatusBadge } from "@/components/common/StatusBadge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { cn } from "@/lib/utils";
import { getRequestErrorMessage } from "@/utils/request";

const formatMoney = (value: number) => `¥${value.toFixed(2)}`;

const inputClass = cn(
  "h-8 w-full rounded-md border border-input bg-background px-2.5 text-sm",
  "placeholder:text-muted-foreground",
  "hover:border-border-strong",
  "focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/30 focus-visible:outline-none",
);

/**
 * 用户管理（仅管理员）。
 *
 * 与「兑换码」分成两个独立页面：这里是"人"，那里是"凭证"。
 * 混在一页用标签页切，用户想「改余额」时得先判断属于哪个标签，反而更慢。
 *
 * 列的顺序：用户 → 角色 → 余额 → 累计充值 → 注册时间 → 操作。
 * 「用户」列**不设过大宽度**：它是唯一需要伸缩的列，给太多会把
 * 右侧数字列挤窄（之前就踩过：名称列吃掉 70% 宽度）。
 */
export const UsersPage = () => {
  const [users, setUsers] = useState<User[]>([]);
  const [error, setError] = useState("");
  const [isLoading, setIsLoading] = useState(true);
  const [reloadKey, setReloadKey] = useState(0);
  const [keyword, setKeyword] = useState("");
  const [page, setPage] = useState(1);
  const [pageSize, setPageSize] = useState(20);

  const [createOpen, setCreateOpen] = useState(false);
  const [balanceUser, setBalanceUser] = useState<User | null>(null);
  const [editUser, setEditUser] = useState<User | null>(null);
  const [passwordUser, setPasswordUser] = useState<User | null>(null);

  const load = useCallback(async () => {
    setIsLoading(true);
    try {
      const response = await listUsers({ pageSize: 200 });
      setUsers(response.data?.list ?? []);
      setError("");
    } catch (requestError) {
      setError(getRequestErrorMessage(requestError));
    } finally {
      setIsLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load, reloadKey]);

  const filtered = useMemo(() => {
    const trimmed = keyword.trim().toLowerCase();
    if (!trimmed) {
      return users;
    }
    return users.filter(
      (user) =>
        user.email.toLowerCase().includes(trimmed) ||
        user.nickname.toLowerCase().includes(trimmed),
    );
  }, [keyword, users]);

  useEffect(() => {
    setPage(1);
  }, [keyword]);

  const paged = useMemo(
    () => filtered.slice((page - 1) * pageSize, page * pageSize),
    [filtered, page, pageSize],
  );

  const columns: Array<Column<User>> = useMemo(
    () => [
      {
        key: "user",
        header: "用户",
        // 宽度收窄：原来 24% 在宽屏上会留出大片空白（昵称/邮箱本来就短），
        // 把空间让给后面的余额、时间列。
        width: "200px",
        // 表头居中、内容左对齐：昵称 + 邮箱是两行长文本，
        // 居中会让两行长度不一、左右都参差，从左起读才整齐。
        bodyAlign: "left",
        render: (user) => (
          <div className="min-w-0">
            <p className="truncate font-medium">{user.nickname || user.email}</p>
            <p className="truncate font-mono text-[11px] text-muted-foreground">
              {user.email}
            </p>
          </div>
        ),
      },
      {
        key: "role",
        header: "角色",
        width: "96px",
        render: (user) => (
          <StatusBadge
            tone={user.role === "admin" ? "info" : "neutral"}
            label={user.role === "admin" ? "管理员" : "普通用户"}
            dot={false}
          />
        ),
      },
      {
        key: "balance",
        header: "余额",
        width: "104px",
        align: "center",
        render: (user) => <span className="tabular-nums">{formatMoney(user.balance)}</span>,
      },
      {
        key: "recharged",
        header: "累计充值",
        width: "112px",
        align: "center",
        hideOnMobile: true,
        render: (user) => (
          <span className="tabular-nums text-muted-foreground">
            {formatMoney(user.totalRecharged)}
          </span>
        ),
      },
      {
        key: "lastLogin",
        header: "最后登录",
        width: "152px",
        hideOnMobile: true,
        render: (user) => (
          <span className="text-xs tabular-nums text-muted-foreground">
            {user.lastLoginAt
              ? new Date(user.lastLoginAt).toLocaleString("zh-CN", {
                  year: "numeric",
                  month: "2-digit",
                  day: "2-digit",
                  hour: "2-digit",
                  minute: "2-digit",
                })
              : "从未登录"}
          </span>
        ),
      },
      {
        key: "created",
        header: "注册时间",
        width: "152px",
        hideOnMobile: true,
        render: (user) => (
          <span className="text-xs tabular-nums text-muted-foreground">
            {new Date(user.createdAt).toLocaleString("zh-CN", {
              year: "numeric",
              month: "2-digit",
              day: "2-digit",
              hour: "2-digit",
              minute: "2-digit",
            })}
          </span>
        ),
      },
      {
        key: "actions",
        header: "操作",
        width: "150px",
        align: "center",
        render: (user) => (
          <RowActions
            actions={[
              {
                label: "调整余额",
                id: "balance",
                icon: Wallet,
                tone: "success",
                onClick: () => setBalanceUser(user),
              },
              {
                label: "编辑",
                id: "edit",
                icon: PencilLine,
                onClick: () => setEditUser(user),
              },
              {
                label: "重置密码",
                id: "password",
                icon: KeyRound,
                tone: "warning",
                onClick: () => setPasswordUser(user),
              },
            ]}
          />
        ),
      },
    ],
    [],
  );

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col gap-3">
      <div className="flex flex-wrap items-center gap-2">
        <div className="relative min-w-0 flex-1 sm:max-w-xs">
          <Search
            className="pointer-events-none absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-muted-foreground"
            aria-hidden="true"
          />
          <input
            type="search"
            value={keyword}
            onChange={(event) => setKeyword(event.target.value)}
            placeholder="搜索昵称或邮箱"
            aria-label="搜索用户"
            className={cn(
              "h-8 w-full rounded-md border border-input bg-background pr-2.5 pl-8 text-sm",
              "placeholder:text-muted-foreground",
              "hover:border-border-strong",
              "focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/30 focus-visible:outline-none",
            )}
          />
        </div>

        <div className="ml-auto flex items-center gap-2">
          <Button
            variant="outline"
            size="sm"
            onClick={() => setReloadKey((value) => value + 1)}
            disabled={isLoading}
          >
            <RefreshCw
              className={cn(isLoading && "animate-spin motion-reduce:animate-none")}
              aria-hidden="true"
            />
            刷新
          </Button>
          <Button size="sm" onClick={() => setCreateOpen(true)}>
            <Plus aria-hidden="true" />
            新建用户
          </Button>
        </div>
      </div>

      {error ? (
        <ErrorState
          message={error}
          onRetry={() => setReloadKey((value) => value + 1)}
          retrying={isLoading}
        />
      ) : null}

      <MyBox
        isLoading={isLoading && users.length === 0}
        text="正在加载用户..."
        className="flex min-h-0 flex-1 flex-col"
      >
        <DataTable
          columns={columns}
          rows={paged}
          rowKey={(user) => String(user.id)}
          empty={
            users.length === 0 ? (
              <EmptyState
                icon={Users}
                title="还没有用户"
                description="可以直接在这里开户，也可以把登录页发给别人让他自己注册。"
                action={
                  <Button size="sm" onClick={() => setCreateOpen(true)}>
                    <Plus aria-hidden="true" />
                    新建用户
                  </Button>
                }
              />
            ) : (
              <EmptyState
                icon={Search}
                title="没有匹配的用户"
                description={`没有昵称或邮箱包含「${keyword}」的用户。`}
                action={
                  <Button variant="outline" size="sm" onClick={() => setKeyword("")}>
                    清除搜索
                  </Button>
                }
              />
            )
          }
        />
      </MyBox>

      <Pagination
        page={page}
        pageSize={pageSize}
        total={filtered.length}
        onPageChange={setPage}
        onPageSizeChange={(size) => {
          setPageSize(size);
          setPage(1);
        }}
        disabled={isLoading}
      />

      <CreateUserDialog
        open={createOpen}
        onOpenChange={setCreateOpen}
        onCreated={() => setReloadKey((value) => value + 1)}
      />

      <AdjustBalanceDialog
        user={balanceUser}
        onClose={() => setBalanceUser(null)}
        onDone={() => setReloadKey((value) => value + 1)}
      />

      <EditUserDialog
        user={editUser}
        onClose={() => setEditUser(null)}
        onDone={() => setReloadKey((value) => value + 1)}
      />

      <ResetPasswordDialog
        user={passwordUser}
        onClose={() => setPasswordUser(null)}
      />
    </div>
  );
};

/** 管理员开户。 */
const CreateUserDialog = ({
  open,
  onOpenChange,
  onCreated,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onCreated: () => void;
}) => {
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [nickname, setNickname] = useState("");
  const [role, setRole] = useState<"user" | "admin">("user");
  const [balance, setBalance] = useState("");
  const [pending, setPending] = useState(false);
  const [error, setError] = useState("");

  useEffect(() => {
    if (!open) {
      setEmail("");
      setPassword("");
      setNickname("");
      setRole("user");
      setBalance("");
      setError("");
    }
  }, [open]);

  const submit = async () => {
    if (!email.trim() || !password) {
      setError("邮箱与密码都不能为空");
      return;
    }

    setPending(true);
    setError("");
    try {
      await createUser({
        email: email.trim(),
        password,
        nickname: nickname.trim(),
        role,
        balance: balance.trim() === "" ? 0 : Number(balance),
      });
      onCreated();
      onOpenChange(false);
    } catch (requestError) {
      setError(getRequestErrorMessage(requestError));
    } finally {
      setPending(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle>新建用户</DialogTitle>
          <DialogDescription>
            直接开户，可指定角色与初始余额。用户之后能自己用兑换码继续充值。
          </DialogDescription>
        </DialogHeader>

        <div className="flex flex-col gap-3">
          <div className="flex flex-col gap-1">
            <label className="text-xs font-medium">邮箱</label>
            <input
              type="email"
              value={email}
              onChange={(event) => setEmail(event.target.value)}
              placeholder="user@example.com"
              className={inputClass}
            />
          </div>

          <div className="flex flex-col gap-1">
            <label className="text-xs font-medium">密码</label>
            <input
              type="text"
              value={password}
              onChange={(event) => setPassword(event.target.value)}
              placeholder="至少 8 位"
              className={cn(inputClass, "font-mono text-xs")}
            />
            <p className="text-[11px] text-muted-foreground">
              明文显示便于转告用户；入库前会哈希。
            </p>
          </div>

          <div className="flex flex-col gap-1">
            <label className="text-xs font-medium">昵称（可选）</label>
            <input
              value={nickname}
              onChange={(event) => setNickname(event.target.value)}
              placeholder="留空则用邮箱前缀"
              className={inputClass}
            />
          </div>

          <div className="grid grid-cols-2 gap-3">
            <div className="flex flex-col gap-1">
              <label className="text-xs font-medium">角色</label>
              <Select value={role} onValueChange={(next) => setRole(next as "user" | "admin")}>
                <SelectTrigger aria-label="角色">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="user">普通用户</SelectItem>
                  <SelectItem value="admin">管理员</SelectItem>
                </SelectContent>
              </Select>
            </div>

            <div className="flex flex-col gap-1">
              <label className="text-xs font-medium">初始余额</label>
              <input
                value={balance}
                onChange={(event) => setBalance(event.target.value)}
                placeholder="0"
                inputMode="decimal"
                className={cn(inputClass, "tabular-nums")}
              />
            </div>
          </div>

          {error ? <ErrorState title="创建失败" message={error} /> : null}
        </div>

        <DialogFooter>
          <Button variant="outline" size="sm" onClick={() => onOpenChange(false)}>
            取消
          </Button>
          <Button size="sm" onClick={() => void submit()} disabled={pending}>
            {pending ? (
              "创建中..."
            ) : (
              <>
                <UserPlus aria-hidden="true" />
                创建
              </>
            )}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
};

/** 调整余额（充值 / 扣减）。 */
const AdjustBalanceDialog = ({
  user,
  onClose,
  onDone,
}: {
  user: User | null;
  onClose: () => void;
  onDone: () => void;
}) => {
  const [amount, setAmount] = useState("");
  const [note, setNote] = useState("");
  const [pending, setPending] = useState(false);
  const [error, setError] = useState("");

  useEffect(() => {
    if (user) {
      setAmount("");
      setNote("");
      setError("");
    }
  }, [user]);

  const submit = async (sign: 1 | -1) => {
    if (!user) {
      return;
    }
    const parsed = Number(amount);
    if (!Number.isFinite(parsed) || parsed <= 0) {
      setError("金额需大于 0");
      return;
    }

    setPending(true);
    setError("");
    try {
      await adjustBalance(
        user.id,
        parsed * sign,
        note.trim() || (sign > 0 ? "管理员充值" : "管理员扣减"),
      );
      onDone();
      onClose();
    } catch (requestError) {
      setError(getRequestErrorMessage(requestError));
    } finally {
      setPending(false);
    }
  };

  return (
    <Dialog open={user !== null} onOpenChange={(next) => !next && onClose()}>
      <DialogContent className="max-w-sm">
        <DialogHeader>
          <DialogTitle>调整余额</DialogTitle>
          <DialogDescription>
            {user ? `${user.nickname || user.email} · 当前 ${formatMoney(user.balance)}` : ""}
          </DialogDescription>
        </DialogHeader>

        <div className="flex flex-col gap-3">
          <div className="flex flex-col gap-1">
            <label className="text-xs font-medium">金额</label>
            <input
              value={amount}
              onChange={(event) => setAmount(event.target.value)}
              placeholder="100"
              inputMode="decimal"
              className={cn(inputClass, "tabular-nums")}
            />
          </div>
          <div className="flex flex-col gap-1">
            <label className="text-xs font-medium">备注（可选）</label>
            <input
              value={note}
              onChange={(event) => setNote(event.target.value)}
              placeholder="例如：活动赠送"
              className={inputClass}
            />
          </div>
          {error ? <ErrorState title="调整失败" message={error} /> : null}
        </div>

        <DialogFooter>
          <Button
            variant="outline"
            size="sm"
            onClick={() => void submit(-1)}
            disabled={pending}
          >
            扣减
          </Button>
          <Button size="sm" onClick={() => void submit(1)} disabled={pending}>
            {pending ? "处理中..." : "充值"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
};

/** 编辑用户：昵称 / 角色 / 启用。 */
const EditUserDialog = ({
  user,
  onClose,
  onDone,
}: {
  user: User | null;
  onClose: () => void;
  onDone: () => void;
}) => {
  const [nickname, setNickname] = useState("");
  const [role, setRole] = useState<"user" | "admin">("user");
  const [enabled, setEnabled] = useState(true);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState("");

  useEffect(() => {
    if (user) {
      setNickname(user.nickname);
      setRole(user.role);
      setEnabled(user.enabled);
      setError("");
    }
  }, [user]);

  const submit = async () => {
    if (!user) {
      return;
    }
    setPending(true);
    setError("");
    try {
      await updateUser(user.id, { nickname: nickname.trim(), role, enabled });
      onDone();
      onClose();
    } catch (requestError) {
      setError(getRequestErrorMessage(requestError));
    } finally {
      setPending(false);
    }
  };

  return (
    <Dialog open={user !== null} onOpenChange={(next) => !next && onClose()}>
      <DialogContent className="max-w-sm">
        <DialogHeader>
          <DialogTitle>编辑用户</DialogTitle>
          <DialogDescription>{user?.email}</DialogDescription>
        </DialogHeader>

        <div className="flex flex-col gap-3">
          <div className="flex flex-col gap-1">
            <label className="text-xs font-medium">昵称</label>
            <input
              value={nickname}
              onChange={(event) => setNickname(event.target.value)}
              className={inputClass}
            />
          </div>
          <div className="flex flex-col gap-1">
            <label className="text-xs font-medium">角色</label>
            <Select value={role} onValueChange={(next) => setRole(next as "user" | "admin")}>
              <SelectTrigger aria-label="角色">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="user">普通用户</SelectItem>
                <SelectItem value="admin">管理员</SelectItem>
              </SelectContent>
            </Select>
          </div>
          <div className="flex flex-col gap-1">
            <label className="text-xs font-medium">状态</label>
            <Select
              value={enabled ? "on" : "off"}
              onValueChange={(next) => setEnabled(next === "on")}
            >
              <SelectTrigger aria-label="状态">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="on">启用</SelectItem>
                <SelectItem value="off">停用（无法登录）</SelectItem>
              </SelectContent>
            </Select>
          </div>
          {error ? <ErrorState title="保存失败" message={error} /> : null}
        </div>

        <DialogFooter>
          <Button variant="outline" size="sm" onClick={onClose}>
            取消
          </Button>
          <Button size="sm" onClick={() => void submit()} disabled={pending}>
            {pending ? "保存中..." : "保存"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
};

/** 重置密码：重置后该用户所有会话失效。 */
const ResetPasswordDialog = ({
  user,
  onClose,
}: {
  user: User | null;
  onClose: () => void;
}) => {
  const [password, setPassword] = useState("");
  const [pending, setPending] = useState(false);
  const [error, setError] = useState("");
  const [done, setDone] = useState(false);

  useEffect(() => {
    if (user) {
      setPassword("");
      setError("");
      setDone(false);
    }
  }, [user]);

  const submit = async () => {
    if (!user) {
      return;
    }
    if (password.length < 8) {
      setError("密码至少 8 位");
      return;
    }

    setPending(true);
    setError("");
    try {
      await resetPassword(user.id, password);
      setDone(true);
    } catch (requestError) {
      setError(getRequestErrorMessage(requestError));
    } finally {
      setPending(false);
    }
  };

  return (
    <Dialog open={user !== null} onOpenChange={(next) => !next && onClose()}>
      <DialogContent className="max-w-sm">
        <DialogHeader>
          <DialogTitle>重置密码</DialogTitle>
          <DialogDescription>
            {user?.nickname || user?.email} · 重置后该用户的所有登录会话会立即失效
          </DialogDescription>
        </DialogHeader>

        <div className="flex flex-col gap-3">
          <div className="flex flex-col gap-1">
            <label className="text-xs font-medium">新密码</label>
            <input
              type="text"
              value={password}
              onChange={(event) => setPassword(event.target.value)}
              placeholder="至少 8 位"
              className={cn(inputClass, "font-mono text-xs")}
            />
            <p className="text-[11px] text-muted-foreground">
              明文显示便于转告用户；入库前会哈希。
            </p>
          </div>

          {error ? <ErrorState title="重置失败" message={error} /> : null}
          {done ? (
            <p className="rounded-md border border-success/30 bg-success/10 px-3 py-2 text-xs text-success">
              密码已重置，该用户需要用新密码重新登录。
            </p>
          ) : null}
        </div>

        <DialogFooter>
          <Button variant="outline" size="sm" onClick={onClose}>
            {done ? "关闭" : "取消"}
          </Button>
          <Button size="sm" onClick={() => void submit()} disabled={pending || done}>
            {pending ? "重置中..." : "确认重置"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
};
