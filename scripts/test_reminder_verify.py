"""Oracle negative controls, never a substitute for OS notification acceptance."""
import importlib.util
from pathlib import Path
import unittest
import tempfile
import copy
import json
import contextlib
import io
from types import SimpleNamespace
spec = importlib.util.spec_from_file_location('reminder_verify', Path(__file__).with_name('reminder-verify.py'))
verify = importlib.util.module_from_spec(spec)
spec.loader.exec_module(verify)


class ReminderOracles(unittest.TestCase):
    def test_exact_owned_paths_cover_spaces_normalization_and_prefix_collision(self):
        path = '/tmp/deck-reminder-owned/Deck Reminder Smoke.app/Contents/MacOS/deck'
        self.assertTrue(verify.same_executable('/private' + path, path))
        self.assertTrue(verify.same_executable(path + ' --debug-logging', path))
        self.assertFalse(verify.same_executable(path + '-other', path))
        self.assertFalse(verify.same_executable(path.replace('owned', 'owned-other'), path))

    def test_cross_trial_build_replacement_and_missing_launch_identity_are_rejected(self):
        a = {'compiledBinarySha256': 'base', 'signedBinarySha256': 'signature-a', 'testedBinarySha256': 'signature-a'}
        b = {'compiledBinarySha256': 'base', 'signedBinarySha256': 'signature-b', 'testedBinarySha256': 'signature-b'}
        self.assertTrue(verify.binary_identity_matches([a, b], 'base'))
        self.assertFalse(verify.binary_identity_matches([a, dict(b, compiledBinarySha256='replacement')], 'base'))
        self.assertFalse(verify.binary_identity_matches([dict(a, testedBinarySha256=None)], 'base'))
        self.assertFalse(verify.binary_identity_matches([dict(a, testedBinarySha256='wrong')], 'base'))

    def test_external_console_lock_stops_next_track_before_native_input(self):
        carrier = object.__new__(verify.LocalCarrier)
        carrier.inventory_only = False
        previous = verify.console_state
        try:
            verify.console_state = lambda: {'locked': True}
            with self.assertRaises(PermissionError):
                carrier.step('native-edit', 2)
        finally:
            verify.console_state = previous

    def test_no_actual_notification_after_quit_is_detected(self):
        for inventory in [{'pending': [{'identifier': 'owned'}], 'delivered': []},
                          {'delivered': [{'identifier': 'wrong', 'deliveredAt': 110}]},
                          {'delivered': [{'identifier': 'owned', 'deliveredAt': 90}]},
                          {'delivered': [{'identifier': 'owned', 'deliveredAt': 110}] * 2}]:
            with self.assertRaises(AssertionError):
                verify.assert_delivered_after_exit(inventory, 'owned', 100, 120)
        self.assertEqual(verify.assert_delivered_after_exit({'delivered': [{'identifier': 'owned', 'deliveredAt': 110}]}, 'owned', 100, 120)['deliveredAt'], 110)



class EvidenceEvaluator(unittest.TestCase):
    """Synthetic evidence tests the oracle, never real product acceptance."""

    def setUp(self):
        self.folder = tempfile.TemporaryDirectory()
        self.addCleanup(self.folder.cleanup)
        self.file = Path(self.folder.name) / 'oracle-fixture.txt'
        self.file.write_text('Synthetic evaluator fixture; not OS evidence.')
        self.refs = verify.evidence_refs([self.file])
        self.resources = {name: {'value': True, 'candidateDiffDigest': 'final',
                                'scope': 'synthetic oracle fixture', 'evidence': self.refs}
                          for name in ['isolation', 'cleanup', 'sharedSafety']}
        self.receipts = []
        for parent, assertions in verify.ASSERTIONS.items():
            for suffix, _, layer in assertions:
                key = parent + '.' + suffix
                for number in range(3 if key in verify.CRITICAL else 1):
                    self.receipts.append({'assertionId': key, 'candidateDiffDigest': 'final',
                                          'runId': str(number), 'environmentId': 'oracle',
                                          'binaryIdentityVerified': True, 'compiledBinarySha256': 'oracle-build',
                                          'signedBinarySha256': 'oracle-signed-'+str(number),
                                          'testedBinarySha256': 'oracle-signed-'+str(number), 'layer': layer,
                                          'status': 'PASS', 'executed': True, 'evidence': self.refs})

    def evaluate(self, receipts=None, resources=None):
        return verify.aggregate(self.receipts if receipts is None else receipts,
                                          'final', self.resources if resources is None else resources,
                                          {'ownWindow': True, 'systemUI': True, 'safeSleep': False})

    def test_offline_empty_message_execution_failure_is_preserved_and_output_is_not_overwritten(self):
        source = Path(self.folder.name)/'original.json'
        source.write_text(json.dumps({'target': 'local', 'candidateDiffDigest': 'final',
                                      'verdict': 'FAIL', 'failure': '', 'trials': []}))
        args = SimpleNamespace(evaluate_saved=source, evidence=Path(self.folder.name)/'output')
        original = source.read_bytes()
        previous = verify.source_digest
        try:
            verify.source_digest = lambda: 'evaluator'
            with contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(verify.evaluate_saved(args), 1)
            self.assertEqual(source.read_bytes(), original)
            output = args.evidence/'unattended-acceptance.json'
            before = output.read_bytes()
            with self.assertRaises(ValueError): verify.evaluate_saved(args)
            self.assertEqual(output.read_bytes(), before)
        finally:
            verify.source_digest = previous

    def test_critical_cross_run_build_replacement_is_a_failure(self):
        receipts = copy.deepcopy(self.receipts)
        item = next(item for item in receipts if item['assertionId'] == 'C2.click')
        item['compiledBinarySha256'] = 'replacement'
        result = self.evaluate(receipts)
        self.assertTrue(result['criticalBuildIdentityFailure'])
        self.assertEqual(result['verdict'], 'FAIL')

    def test_partial_receipt_retains_covered_subset_without_certifying_parent(self):
        receipts = copy.deepcopy(self.receipts)
        for item in receipts:
            if item['assertionId'] == 'A2.preview':
                item['status'] = 'PARTIAL PASS'
                item['coveredSubset'] = 'Only the zone string was observed'
        result = self.evaluate(receipts)
        row = next(row for row in result['matrix'] if row['id'] == 'A2')
        self.assertEqual(row['status'], 'PARTIAL PASS')
        self.assertIn('A2.preview', row['remainingAssertions'])
        self.assertEqual(result['scopes']['core']['verdict'], 'BLOCKED')

    def test_missing_evidence_path_is_not_silently_dropped(self):
        refs = verify.evidence_refs([self.file, Path(self.folder.name)/'missing.json'])
        self.assertEqual(len(refs), 2)
        self.assertFalse(verify.references_valid(refs))

    def test_no_started_resources_is_unknown_not_a_false_isolation_failure(self):
        result = self.evaluate([], {})
        self.assertEqual(result['verdict'], 'BLOCKED')
        self.assertIsNone(result['isolationVerified'])
        self.assertFalse(result['requiredTestsExecuted'])

    def test_binary_identity_mismatch_cannot_certify_execution(self):
        receipts = copy.deepcopy(self.receipts)
        for item in receipts:
            if item['layer'] != 'logic': item['binaryIdentityVerified'] = False
        result = self.evaluate(receipts)
        self.assertEqual(result['scopes']['core']['verdict'], 'BLOCKED')
        self.assertTrue(any(item['reason'] == 'unknown/mismatched binary identity' for item in result['excludedReceipts']))

    def test_complete_core_pass_is_reachable_without_sleep_readiness(self):
        result = self.evaluate()
        self.assertEqual(result['scopes']['core']['verdict'], 'PASS')
        self.assertEqual(result['verdict'], 'PASS')
        self.assertFalse(result['readiness']['platform']['safeSleep'])
        self.assertEqual(len(result['completeCriticalChains']), 3)

    def test_each_missing_core_system_interaction_blocks_core(self):
        system = [item['assertionId'] for item in self.receipts if item['layer'] == 'system']
        for key in set(system):
            with self.subTest(assertion=key):
                result = self.evaluate([item for item in self.receipts if item['assertionId'] != key])
                self.assertEqual(result['scopes']['core']['verdict'], 'BLOCKED')

    def test_platform_unverified_remains_visible_and_does_not_block_core(self):
        result = self.evaluate([item for item in self.receipts if item['assertionId'] != 'E5.sleep'])
        self.assertEqual(result['scopes']['core']['verdict'], 'PASS')
        self.assertEqual(result['scopes']['platform']['verdict'], 'BLOCKED')
        self.assertEqual(result['verdict'], 'BLOCKED')
        self.assertIn('E5.sleep', result['scopes']['platform']['remainingAssertions'])

    def test_valid_failure_outranks_missing_evidence_and_same_run_success(self):
        receipts = [item for item in self.receipts if item['layer'] != 'platform']
        receipts.append(dict(receipts[0], status='FAIL'))
        result = self.evaluate(receipts)
        self.assertEqual(result['verdict'], 'FAIL')
        self.assertEqual(result['scopes']['core']['verdict'], 'FAIL')
        self.assertEqual(result['scopes']['platform']['verdict'], 'BLOCKED')

    def test_history_duplicate_unexecuted_wrong_layer_and_unknown_binary_cannot_fill_runs(self):
        key = 'C2.click'
        rest = [item for item in self.receipts if item['assertionId'] != key]
        template = next(item for item in self.receipts if item['assertionId'] == key)
        variants = [[dict(template, candidateDiffDigest='historical')] * 3,
                    [template] * 3, [dict(template, executed=False)] * 3,
                    [dict(template, layer='wk')] * 3,
                    [dict(template, binaryIdentityVerified=None)] * 3]
        for variant in variants:
            with self.subTest(variant=variant[0]):
                self.assertEqual(self.evaluate(rest + variant)['scopes']['core']['verdict'], 'BLOCKED')

    def test_independent_partial_runs_and_environments_cannot_be_stitched(self):
        receipts = copy.deepcopy(self.receipts)
        for item in receipts:
            if item['assertionId'] == 'C2.click': item['environmentId'] = 'other'
        self.assertEqual(self.evaluate(receipts)['scopes']['core']['verdict'], 'BLOCKED')
        for item in receipts:
            if item['assertionId'] == 'C2.click': item['environmentId'] = 'oracle'; item['runId'] += '-other'
        self.assertEqual(self.evaluate(receipts)['scopes']['core']['verdict'], 'BLOCKED')

    def test_unknown_isolation_and_cleanup_do_not_establish_shared_safety(self):
        for name in ['isolation', 'sharedSafety']:
            resources = copy.deepcopy(self.resources)
            resources[name] = {'value': True}  # An unsupported flag is not evidence.
            result = self.evaluate(resources=resources)
            self.assertEqual(result['resources'][name]['status'], 'UNKNOWN')
            self.assertEqual(result['verdict'], 'BLOCKED')
            if name == 'isolation': self.assertEqual(result['scopes']['core']['verdict'], 'BLOCKED')
            else: self.assertIsNone(result['sharedResourcesSafe'])

    def test_changed_evidence_is_rejected_and_resource_failure_outranks_blocker(self):
        self.file.write_text('changed fixture')
        self.assertEqual(self.evaluate()['verdict'], 'BLOCKED')
        self.refs[:] = verify.evidence_refs([self.file])
        resources = copy.deepcopy(self.resources)
        resources['cleanup']['value'] = False
        receipts = [item for item in self.receipts if item['layer'] != 'platform']
        self.assertEqual(self.evaluate(receipts, resources)['verdict'], 'FAIL')

    def test_native_rows_are_derived_and_can_pass_when_all_subassertions_pass(self):
        result = self.evaluate()
        self.assertTrue(all(row['status'] == 'PASS' for row in result['nativeMatrix']))
        partial = self.evaluate([item for item in self.receipts if item['assertionId'] != 'C5.agent'])
        row = next(item for item in partial['nativeMatrix'] if item['id'] == 'N5')
        self.assertEqual(row['status'], 'PARTIAL PASS')
        self.assertIn('C5.agent', row['remainingAssertions'])


if __name__ == '__main__':
    unittest.main()
