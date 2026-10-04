/**
 * `/` is the morning report. File-based routing: this file is the route; the page
 * itself is a client component because the build is a static export (no Node server
 * in prod) and everything here is driven by fetches to the axum API.
 */
import { ReportApp } from '@/report/ReportApp'

export default function Page() {
  return <ReportApp />
}
