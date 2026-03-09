export const pikachatPluginConfigSchema = {
  type: "object",
  additionalProperties: false,
  properties: {
    relays: {
      type: "array",
      items: { type: "string" },
      minItems: 1,
    },
    stateDir: { type: "string" },
    daemonCmd: { type: "string" },
    daemonArgs: {
      type: "array",
      items: { type: "string" },
    },
    daemonVersion: { type: "string" },
    daemonBackend: {
      type: "string",
      enum: ["native", "acp"],
      default: "native",
    },
    daemonAcpExec: { type: "string" },
    daemonAcpCwd: { type: "string" },
    sidecarCmd: { type: "string" },
    sidecarArgs: {
      type: "array",
      items: { type: "string" },
    },
    sidecarVersion: { type: "string" },
    autoAcceptWelcomes: {
      type: "boolean",
      default: true,
    },
    groupPolicy: {
      type: "string",
      enum: ["allowlist", "open"],
      default: "allowlist",
    },
    groupAllowFrom: {
      type: "array",
      items: { type: "string" },
    },
    groups: {
      type: "object",
      additionalProperties: {
        type: "object",
        additionalProperties: false,
        properties: {
          name: { type: "string" },
        },
      },
    },
  },
  required: [],
} as const;
