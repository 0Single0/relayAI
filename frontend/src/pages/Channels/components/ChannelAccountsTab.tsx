import { Plus, UserRound } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { listAccounts } from "@/api/accounts";
import type { Account, Channel } from "@/api/types";
import { DataTable, type Column } from "@/components/common/DataTable";
import { EmptyState } from "@/components/common/EmptyState";
import { ErrorState } from "@/components/common/ErrorState";
import { MyBox } from "@/components/common/MyBox";
import { Button } from "@/components/ui/button";
import { CreateAccountDrawer } from "@/pages/Accounts/components/CreateAccountDrawer";
import { AccountStatusCell } from "@/pages/Accounts/components/AccountStatusCell";
import { getRequestErrorMessage } from "@/utils/request";

type ChannelAccountsTabProps = {
  channel: Channel;
};

export const ChannelAccountsTab = ({ channel }: ChannelAccountsTabProps) => {
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [error, setError] = useState("");
  const [isLoading, setIsLoading] = useState(true);
  const [addOpen, setAddOpen] = useState(false);
  const [reloadKey, setReloadKey] = useState(0);

  const load = useCallback(async () => {
    setIsLoading(true);
    try {
      const response = await listAccounts({ channelId: channel.id, pageSize: 100 });
      setAccounts(response.data?.list ?? []);
      setError("");
    } catch (requestError) {
      setError(getRequestErrorMessage(requestError));
    } finally {
      setIsLoading(false);
    }
  }, [channel.id]);

  useEffect(() => {
    void load();
  }, [load, reloadKey]);

  const columns: Array<Column<Account>> = [
    {
      key: "account",
      header: "账号",
      render: (account) => (
        <Link
          to={`/accounts/${account.id}`}
          className="block min-w-0 truncate font-medium hover:text-primary"
        >
          {account.nickname || account.uid}
          <span className="ml-1.5 font-mono text-[11px] font-normal text-muted-foreground">
            {account.uid}
          </span>
        </Link>
      ),
    },
    {
      key: "status",
      header: "状态",
      width: "140px",
      render: (account) => <AccountStatusCell account={account} />,
    },
    {
      key: "inflight",
      header: "在飞",
      width: "70px",
      align: "right",
      hideOnMobile: true,
      render: (account) => (
        <span className="tabular-nums text-muted-foreground">
          {account.runtime ? `${account.runtime.inflight}/${account.maxInflight ?? "∞"}` : "—"}
        </span>
      ),
    },
    {
      key: "models",
      header: "模型白名单",
      width: "140px",
      hideOnMobile: true,
      render: (account) => (
        <span className="text-xs text-muted-foreground">
          {account.modelAllow.length === 0 ? "全部" : `${account.modelAllow.length} 个`}
        </span>
      ),
    },
  ];

  return (
    <div className="flex min-h-0 flex-col gap-3">
      <div className="flex items-center gap-2">
        <span className="text-xs text-muted-foreground">
          共 {accounts.length} 个账号
        </span>
        <Button size="sm" className="ml-auto" onClick={() => setAddOpen(true)}>
          <Plus aria-hidden="true" />
          添加账号
        </Button>
      </div>

      {error ? (
        <ErrorState
          message={error}
          onRetry={() => setReloadKey((value) => value + 1)}
          retrying={isLoading}
        />
      ) : null}

      <MyBox
        isLoading={isLoading && accounts.length === 0}
        text="正在加载账号..."
        className="flex min-h-0 flex-1 flex-col"
      >
        <DataTable
          columns={columns}
          rows={accounts}
          rowKey={(account) => String(account.id)}
          empty={
            <EmptyState
              icon={UserRound}
              title="该渠道还没有账号"
              description="添加一个账号后，网关才能用它发起上游请求。"
              action={
                <Button size="sm" onClick={() => setAddOpen(true)}>
                  <Plus aria-hidden="true" />
                  添加账号
                </Button>
              }
            />
          }
        />
      </MyBox>

      <CreateAccountDrawer
        open={addOpen}
        defaultChannelId={channel.id}
        onOpenChange={setAddOpen}
        onCreated={() => {
          setAddOpen(false);
          setReloadKey((value) => value + 1);
        }}
      />
    </div>
  );
};
