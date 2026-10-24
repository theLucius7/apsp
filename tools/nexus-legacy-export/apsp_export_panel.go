// Copy into internal/panel/ next to apsp_export_test.go's target; see that
// file. Exposes the legacy weak decoder used by Client.GetConfig unchanged.
package panel

func ApspDecodeWeakRaw(raw map[string]interface{}, cfg *NodeConfig) error {
	return decodeWeakRaw(raw, cfg)
}
