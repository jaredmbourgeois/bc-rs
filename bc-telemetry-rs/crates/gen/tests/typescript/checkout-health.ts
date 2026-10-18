// One dashboard, built with Grafana's Foundation SDK from the canonical tree's generated query
// handles (crates/gen/tests/fixtures/canonical/generated/telemetry/grafana/queries.ts), printed as
// JSON. The generator's suite type-checks it, runs it, and holds what it prints to Grafana's own
// dashboard schema (tests/suite/typescript.rs).

import * as dashboard from "@grafana/grafana-foundation-sdk/dashboard";
import * as logs from "@grafana/grafana-foundation-sdk/logs";
import * as stat from "@grafana/grafana-foundation-sdk/stat";
import * as timeseries from "@grafana/grafana-foundation-sdk/timeseries";

import { dataSource, queries } from "./queries";

// The one thing this program needs from Node, declared rather than fetched with all of Node's types.
declare const console: { log(text: string): void };

const latency = queries.storeServer.queries.checkoutP95;
const orders = queries.storeServer.queries.ordersPerSecond;
const failedOrders = queries.storeServer.records.orderFailed;

const built = new dashboard.DashboardBuilder("Checkout health, from TypeScript")
  .uid("typescript-checkout-health")
  .withRow(new dashboard.RowBuilder("Latency"))
  .withPanel(
    new timeseries.PanelBuilder()
      .title("p95 latency by route")
      .description(latency.description)
      .datasource(dataSource)
      .withTarget(latency)
      .unit(latency.unit),
  )
  .withPanel(
    new stat.PanelBuilder()
      .title("Orders per second")
      .description(orders.description)
      .datasource(dataSource)
      .withTarget(orders)
      .unit(orders.unit),
  )
  .withRow(new dashboard.RowBuilder("Failures"))
  .withPanel(
    new logs.PanelBuilder()
      .title("Failed orders")
      .description(failedOrders.description)
      .datasource(dataSource)
      .withTarget(failedOrders),
  )
  .build();

console.log(JSON.stringify(built, null, 2));
