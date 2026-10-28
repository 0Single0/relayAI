import { ArrowLeft, FileQuestion } from "lucide-react";
import { Link } from "react-router-dom";
import { buttonVariants } from "@/components/ui/button";
import { cn } from "@/lib/utils";

export const NotFoundPage = () => (
  <div className="flex min-h-[50vh] flex-col items-center justify-center text-center">
    <div className="flex size-12 items-center justify-center rounded-full border bg-muted/50">
      <FileQuestion
        className="size-5 text-muted-foreground"
        aria-hidden="true"
      />
    </div>
    <p className="mt-4 text-xs font-medium tracking-widest text-muted-foreground uppercase">
      404
    </p>
    <h2 className="mt-1 text-lg font-medium">页面不存在</h2>
    <p className="mt-1 max-w-sm text-sm text-muted-foreground">
      该地址尚未注册，或对应的业务页面还未开始实现。
    </p>
    <Link
      to="/"
      className={cn(buttonVariants({ variant: "outline" }), "mt-5")}
    >
      <ArrowLeft aria-hidden="true" />
      返回项目概览
    </Link>
  </div>
);
