# ---------------------------------------------------------------------------
# Global HTTPS Load Balancer — fronts the GKE gateway pods.
# Cloud Armor WAF policy provides basic DDoS + OWASP top-10 protection.
# Google-managed TLS cert auto-renews.
# ---------------------------------------------------------------------------

# Static global IP
resource "google_compute_global_address" "gateway" {
  name = "lynxx-gateway-ip"
}

# Google-managed TLS certificate
resource "google_compute_managed_ssl_certificate" "gateway" {
  name = "lynxx-gateway-cert"

  managed {
    domains = [var.domain]
  }
}

# ---- Backend config --------------------------------------------------------
# The NEG (Network Endpoint Group) is created automatically by GKE when you
# annotate the gateway Service with:
#   cloud.google.com/neg: '{"ingress": true}'
# We reference it here by naming convention after helm deploy.
# For now we define a backend service that the Ingress object will populate.

resource "google_compute_backend_service" "gateway" {
  name                  = "lynxx-gateway-backend"
  protocol              = "HTTP"
  port_name             = "proxy"
  load_balancing_scheme = "EXTERNAL_MANAGED"
  timeout_sec           = 30
  enable_cdn            = false

  health_checks = [google_compute_health_check.gateway.id]

  security_policy = google_compute_security_policy.gateway.id

  log_config {
    enable      = true
    sample_rate = 0.1
  }

  # Backends are added by GKE NEG auto-provisioning after the gateway Service
  # is annotated.  Leave empty here; Terraform will manage it once NEGs exist.
}

resource "google_compute_health_check" "gateway" {
  name = "lynxx-gateway-hc"

  http_health_check {
    port         = 9001
    request_path = "/status"
  }

  check_interval_sec  = 5
  timeout_sec         = 5
  healthy_threshold   = 2
  unhealthy_threshold = 3
}

# ---- URL map ---------------------------------------------------------------
resource "google_compute_url_map" "gateway" {
  name            = "lynxx-gateway-urlmap"
  default_service = google_compute_backend_service.gateway.id
}

resource "google_compute_url_map" "gateway_redirect" {
  name = "lynxx-gateway-http-redirect"

  default_url_redirect {
    https_redirect         = true
    redirect_response_code = "MOVED_PERMANENTLY_DEFAULT"
    strip_query            = false
  }
}

# ---- Target proxies + forwarding rules ------------------------------------
resource "google_compute_target_https_proxy" "gateway" {
  name             = "lynxx-gateway-https-proxy"
  url_map          = google_compute_url_map.gateway.id
  ssl_certificates = [google_compute_managed_ssl_certificate.gateway.id]
}

resource "google_compute_target_http_proxy" "gateway_redirect" {
  name    = "lynxx-gateway-http-proxy"
  url_map = google_compute_url_map.gateway_redirect.id
}

resource "google_compute_global_forwarding_rule" "gateway_https" {
  name                  = "lynxx-gateway-https"
  target                = google_compute_target_https_proxy.gateway.id
  port_range            = "443"
  ip_address            = google_compute_global_address.gateway.address
  load_balancing_scheme = "EXTERNAL_MANAGED"
}

resource "google_compute_global_forwarding_rule" "gateway_http" {
  name                  = "lynxx-gateway-http"
  target                = google_compute_target_http_proxy.gateway_redirect.id
  port_range            = "80"
  ip_address            = google_compute_global_address.gateway.address
  load_balancing_scheme = "EXTERNAL_MANAGED"
}

# ---- Cloud Armor WAF -------------------------------------------------------
resource "google_compute_security_policy" "gateway" {
  name        = "lynxx-gateway-armor"
  description = "Cloud Armor WAF for the Lynxx API gateway"

  # OWASP ModSecurity CRS — preconfigured rule set
  rule {
    action   = "deny(403)"
    priority = 1000

    match {
      expr {
        expression = "evaluatePreconfiguredExpr('xss-v33-stable')"
      }
    }
    description = "XSS"
  }

  rule {
    action   = "deny(403)"
    priority = 1001

    match {
      expr {
        expression = "evaluatePreconfiguredExpr('sqli-v33-stable')"
      }
    }
    description = "SQLi"
  }

  rule {
    action   = "deny(403)"
    priority = 1002

    match {
      expr {
        expression = "evaluatePreconfiguredExpr('rfi-canary')"
      }
    }
    description = "Remote file inclusion"
  }

  # Rate limit per IP — backstop for clients that bypass the gateway plugin
  rule {
    action   = "throttle"
    priority = 2000

    match {
      versioned_expr = "SRC_IPS_V1"
      config {
        src_ip_ranges = ["*"]
      }
    }

    rate_limit_options {
      rate_limit_threshold {
        count        = 1000
        interval_sec = 60
      }
      conform_action = "allow"
      exceed_action  = "deny(429)"
      enforce_on_key = "IP"
    }
    description = "Per-IP rate limit backstop"
  }

  # Default allow
  rule {
    action   = "allow"
    priority = 2147483647

    match {
      versioned_expr = "SRC_IPS_V1"
      config {
        src_ip_ranges = ["*"]
      }
    }
    description = "Default allow"
  }

  adaptive_protection_config {
    layer_7_ddos_defense_config {
      enable = true
    }
  }
}
