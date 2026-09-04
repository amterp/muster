import Foundation

/// Where Muster writes down the daemons it starts.
///
/// The other half of `muster window`. That says which daemons this window is attached to, which
/// is what makes ending this window's sessions deliberate; this is what lets `muster daemons`
/// answer the question nothing else can - which daemons are on this machine that no window is
/// attached to. Those are the ones that accumulate, and telling them apart matters: of twenty
/// alive on one machine, nineteen held nothing and one held somebody's live agent.
///
/// A directory rather than a file, one record per daemon, so two windows starting daemons on two
/// sockets never write the same file. That is what lets this be a plain write where `panes.toml`
/// beside it needs a lock.
///
/// Beside the arrangements and the names, under Muster's own home, so `MUSTER_HOME` moves the
/// lot and a test pointed at a scratch home gets a scratch record without being told about this
/// file.
///
/// `nil` when the environment says nothing about where home is. A real state rather than a
/// failure: nothing is written down, and `muster daemons` says that is why rather than reporting
/// a machine with no daemons on it.
public func daemonRecordsPath(environment: [String: String] = ProcessInfo.processInfo.environment)
  -> String?
{
  musterHome(environment: environment)?.appendingPathComponent("state/daemons").path
}
