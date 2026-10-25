// Export the effective sing-box configuration that the legacy Nexus node
// (xboard-node) builds for each node, using its own builder code.
//
// Copy this file into internal/kernel/singbox/ and apsp_export_panel.go into
// internal/panel/ of the exact xboard-node source that runs on the host, then
// run in an isolated container where the host's /etc/xboard-node (or
// /etc/nexus-agent) files are mounted read-only at their original paths:
//
//	APSP_LEGACY_CONFIG=/etc/xboard-node/config.yml \
//	APSP_EXPORT_INPUT=/private/host-request.json \
//	APSP_EXPORT_OUTPUT=/private/host-effective.json \
//	go test ./internal/kernel/singbox/ -run TestApspExportEffectiveConfig -count=1
//
// The request is {"nodes":[{"legacy_node_id":N,"machine_id":M,"type":"vless",
// "node_config":{...panel node config...}}]}. A missing custom_config file is
// an error here even though the legacy node skips it, so nothing is silently
// omitted from the migrated configuration. Output files hold node keys; keep
// them in a mode-0700 directory.
// Hysteria2 file-mode certificates also need read-only mounts at their original
// paths. The exporter preserves their exact bytes, including expired material
// for explicitly disabled inventory; it does not authorize a live listener.
package singbox

import (
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/tls"
	"crypto/x509"
	"encoding/json"
	"encoding/pem"
	"errors"
	"fmt"
	"io"
	"math/big"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/cedar2025/xboard-node/internal/config"
	"github.com/cedar2025/xboard-node/internal/kernel"
	"github.com/cedar2025/xboard-node/internal/model"
	"github.com/cedar2025/xboard-node/internal/panel"
	"gopkg.in/yaml.v3"
)

type apspExportNode struct {
	LegacyNodeID int            `json:"legacy_node_id"`
	MachineID    int            `json:"machine_id"`
	Type         string         `json:"type"`
	NodeConfig   map[string]any `json:"node_config"`
}

type apspExportResult struct {
	LegacyNodeID  int    `json:"legacy_node_id"`
	Configuration M      `json:"configuration,omitempty"`
	Error         string `json:"error,omitempty"`
}

// The placeholder user only makes the inbound shape complete; apsp replaces
// the user list with the entitled members at publication time.
const apspPlaceholderUUID = "00000000-0000-4000-8000-000000000001"

func apspExportOne(instances []*config.Config, node apspExportNode) (M, error) {
	var instance *config.Config
	for _, candidate := range instances {
		if candidate.IsMachineMode() && candidate.Machine.MachineID == node.MachineID {
			if instance != nil {
				return nil, errors.New("machine is configured by more than one instance")
			}
			instance = candidate
		}
	}
	if instance == nil {
		return nil, errors.New("no local instance manages this machine")
	}
	nodeCfg := instance.ExpandMachineNode(node.LegacyNodeID, node.Type)
	if nodeCfg.Kernel.Type != "singbox" {
		return nil, fmt.Errorf("kernel %q is not sing-box", nodeCfg.Kernel.Type)
	}
	if path := nodeCfg.Kernel.CustomConfig; path != "" {
		if _, err := os.Stat(path); err != nil {
			return nil, fmt.Errorf("custom_config %s is not available: %w", path, err)
		}
	}
	var nc panel.NodeConfig
	if err := panel.ApspDecodeWeakRaw(node.NodeConfig, &nc); err != nil {
		return nil, fmt.Errorf("decode panel config: %w", err)
	}
	if resolved := model.ResolveKernelForTransport(nc.Network, nodeCfg.Kernel.Type); resolved != nodeCfg.Kernel.Type {
		return nil, fmt.Errorf("transport %q switches the legacy kernel to %s", nc.Network, resolved)
	}
	spec, err := model.NodeSpecFromPanelValidated(&nc, nodeCfg.Kernel)
	if err != nil {
		return nil, fmt.Errorf("legacy validation: %w", err)
	}
	if spec.RoutingValidationError != "" {
		return nil, fmt.Errorf("legacy routing validation: %s", spec.RoutingValidationError)
	}
	users := []model.UserSpec{{ID: 1, UUID: apspPlaceholderUUID}}
	var certificate kernel.TLSCert
	if spec.Protocol == "hysteria" && spec.Version == 2 {
		cfg := nodeCfg.Cert
		if spec.CertConfig != nil {
			// The running legacy service gives panel certificate settings priority.
			cfg = *spec.CertConfig
		}
		certificate, err = apspStaticCertificate(cfg)
		if err != nil {
			return nil, err
		}
	}
	return buildConfig(nodeCfg.Kernel, spec, users, certificate), nil
}

func apspCertificateFile(path string, limit int64) ([]byte, error) {
	if !filepath.IsAbs(path) {
		return nil, errors.New("certificate file must use its original absolute path")
	}
	file, err := os.Open(path)
	if err != nil {
		return nil, errors.New("certificate file is unavailable in the read-only snapshot")
	}
	defer file.Close()
	info, err := file.Stat()
	if err != nil || !info.Mode().IsRegular() || info.Size() <= 0 || info.Size() > limit {
		return nil, errors.New("certificate file is not a bounded regular file")
	}
	data, err := io.ReadAll(io.LimitReader(file, limit+1))
	if err != nil || int64(len(data)) > limit {
		return nil, errors.New("certificate file exceeds its bound")
	}
	return data, nil
}

// Preserve existing static material only. Exporting never requests a new
// certificate, renews one, or substitutes a self-signed identity.
func apspStaticCertificate(cfg config.CertConfig) (kernel.TLSCert, error) {
	mode := strings.ToLower(strings.TrimSpace(cfg.CertMode))
	if mode == "" {
		switch {
		case cfg.AutoTLS:
			mode = "http"
		case cfg.CertContent != "" && cfg.KeyContent != "":
			mode = "content"
		case cfg.CertFile != "" && cfg.KeyFile != "":
			mode = "file"
		}
	}
	var result kernel.TLSCert
	var err error
	switch mode {
	case "file":
		result.CertPEM, err = apspCertificateFile(cfg.CertFile, 65536)
		if err == nil {
			result.KeyPEM, err = apspCertificateFile(cfg.KeyFile, 16384)
		}
	case "content":
		result.CertPEM, result.KeyPEM = []byte(cfg.CertContent), []byte(cfg.KeyContent)
	default:
		return result, errors.New("certificate mode requires explicit material export")
	}
	if err != nil {
		return result, err
	}
	if len(result.CertPEM) > 65536 || len(result.KeyPEM) > 16384 {
		return result, errors.New("certificate material exceeds its bound")
	}
	if _, err := tls.X509KeyPair(result.CertPEM, result.KeyPEM); err != nil {
		return result, errors.New("certificate and private key do not form a valid pair")
	}
	return result, nil
}

// Machine tokens normally come from credentials.env. They do not influence the
// generated kernel configuration, so the export never reads them: every
// referenced token_env variable gets a placeholder when it is unset.
func apspPlaceholderTokens(path string) error {
	data, err := os.ReadFile(path)
	if err != nil {
		return err
	}
	var document any
	if err := yaml.Unmarshal(data, &document); err != nil {
		return err
	}
	var walk func(any)
	walk = func(value any) {
		switch typed := value.(type) {
		case map[string]any:
			for key, item := range typed {
				if name, ok := item.(string); ok && key == "token_env" && name != "" && os.Getenv(name) == "" {
					os.Setenv(name, "apsp-export-placeholder")
				}
				walk(item)
			}
		case []any:
			for _, item := range typed {
				walk(item)
			}
		}
	}
	walk(document)
	return nil
}

func TestApspExportEffectiveConfig(t *testing.T) {
	input, output, legacy := os.Getenv("APSP_EXPORT_INPUT"), os.Getenv("APSP_EXPORT_OUTPUT"), os.Getenv("APSP_LEGACY_CONFIG")
	if input == "" || output == "" || legacy == "" {
		t.Skip("set APSP_LEGACY_CONFIG, APSP_EXPORT_INPUT and APSP_EXPORT_OUTPUT")
	}
	if err := apspPlaceholderTokens(legacy); err != nil {
		t.Fatalf("read legacy config: %v", err)
	}
	root, err := config.LoadRoot(legacy)
	if err != nil {
		t.Fatalf("load legacy config: %v", err)
	}
	instances, err := root.NormalizeInstances()
	if err != nil {
		t.Fatalf("normalize legacy instances: %v", err)
	}
	data, err := os.ReadFile(input)
	if err != nil {
		t.Fatal(err)
	}
	var request struct {
		Nodes []apspExportNode `json:"nodes"`
	}
	if err := json.Unmarshal(data, &request); err != nil {
		t.Fatal(err)
	}
	results := make([]apspExportResult, 0, len(request.Nodes))
	for _, node := range request.Nodes {
		result := apspExportResult{LegacyNodeID: node.LegacyNodeID}
		if cfg, err := apspExportOne(instances, node); err != nil {
			result.Error = err.Error()
		} else {
			result.Configuration = cfg
		}
		results = append(results, result)
	}
	encoded, err := json.Marshal(results)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(output, encoded, 0o600); err != nil {
		t.Fatal(err)
	}
}

func TestApspStaticCertificatePreservesExpiredMaterialForDisabledInventory(t *testing.T) {
	key, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	template := &x509.Certificate{SerialNumber: big.NewInt(1), DNSNames: []string{"fixture.example.invalid"},
		NotBefore: time.Unix(1, 0), NotAfter: time.Unix(2, 0)}
	der, err := x509.CreateCertificate(rand.Reader, template, template, &key.PublicKey, key)
	if err != nil {
		t.Fatal(err)
	}
	encodedKey, err := x509.MarshalPKCS8PrivateKey(key)
	if err != nil {
		t.Fatal(err)
	}
	certificate := pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: der})
	privateKey := pem.EncodeToMemory(&pem.Block{Type: "PRIVATE KEY", Bytes: encodedKey})
	directory := t.TempDir()
	certPath, keyPath := filepath.Join(directory, "certificate.pem"), filepath.Join(directory, "key.pem")
	for path, data := range map[string][]byte{certPath: certificate, keyPath: privateKey} {
		if err := os.WriteFile(path, data, 0o600); err != nil {
			t.Fatal(err)
		}
	}
	result, err := apspStaticCertificate(config.CertConfig{CertMode: "file", CertFile: certPath, KeyFile: keyPath})
	if err != nil || string(result.CertPEM) != string(certificate) || string(result.KeyPEM) != string(privateKey) {
		t.Fatal("static export must preserve the existing pair exactly", err)
	}
	for _, cfg := range []config.CertConfig{
		{CertMode: "http"},
		{CertMode: "file", CertFile: certPath, KeyFile: certPath},
		{CertMode: "content", CertContent: strings.Repeat("x", 65537), KeyContent: string(privateKey)},
	} {
		if _, err := apspStaticCertificate(cfg); err == nil {
			t.Fatal("unsupported or invalid certificate material was accepted")
		}
	}
}
