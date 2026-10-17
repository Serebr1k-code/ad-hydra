# Service port mapping

For a service running on the same host as Hydra, keep the original client-facing port on **hydra-proxy** and move the application to a separate loopback port. For example:

```text
clients → Hydra 0.0.0.0:8000 → application 127.0.0.1:18000
```

1. Change the application's bind configuration to `127.0.0.1:18000` and restart it using its existing deployment mechanism. Verify the backend responds and the original public port is free.
2. In **Services**, add or edit the service. Enter `0.0.0.0:8000` as the public listener and `127.0.0.1:18000` as the application backend, with the correct transport and protocol profile.
3. Enable the listener and save. Check desired/applied state and any apply error, then verify the original client-facing endpoint.

Saving configures Hydra's listener and forwarding destination. Hydra does not edit or restart the defended application, manage containers, or change host firewall rules. Prepare the application before saving an enabled service; a port already occupied by the application cannot be bound by Proxy. Uncheck **Enable proxy listener** to store a configuration before enabling it.

IPv6 endpoints use brackets, such as `[::]:8000` and `[::1]:18000`. The form rejects identical endpoints and obvious wildcard-to-loopback routes back into the listener. Core validates the full service snapshot, including conflicts with other listeners. External backends remain supported and may retain their original port when hosted elsewhere.

Ordinary reverse proxying changes the source IP seen by the application. The initial deployment mode does not preserve the original client source address at the backend.
