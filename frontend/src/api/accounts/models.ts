export type CustomModelDefinition = {
  id: string;
  display_name?: string;
  context_length?: number | null;
  max_output_tokens?: number | null;
  capabilities?: { vision?: boolean; tool_calls?: boolean; reasoning?: boolean; declared_tools?: boolean; declared_vision?: boolean };
  reasoning_efforts?: string[];
  reasoning_effort_map?: Record<string, string>;
  reasoning_default_effort?: string | null;
};
