#!/usr/bin/env python3
"""Offline path-containment tests using only temporary, synthetic files."""

from pathlib import Path
import tempfile
import io
import subprocess
import unittest
from unittest.mock import patch

import demo
from demo_flow import Flow


class BuildPrerequisiteTests(unittest.TestCase):
    def test_missing_docker_fails_before_storage_or_build(self):
        with patch('sys.argv', ['demo.py']), patch('sys.stderr', new_callable=io.StringIO) as stderr, \
                patch.object(demo.shutil, 'which', return_value=None), \
                patch.object(demo, 'safe_path') as storage, patch.object(demo.subprocess, 'run') as run:
            with self.assertRaises(SystemExit) as error:
                demo.main()
            self.assertEqual(error.exception.code, 2)
            self.assertIn('requires the docker CLI', stderr.getvalue())
            storage.assert_not_called()
            run.assert_not_called()

    def test_dirty_tree_fails_before_storage_or_build(self):
        result = subprocess.CompletedProcess([], 0, stdout=' M contracts/wallet/Cargo.toml\n')
        with patch('sys.argv', ['demo.py']), patch('sys.stderr', new_callable=io.StringIO) as stderr, \
                patch.object(demo.shutil, 'which', return_value='/usr/bin/docker'), \
                patch.object(demo, 'safe_path') as storage, \
                patch.object(demo.subprocess, 'run', return_value=result) as run:
            with self.assertRaises(SystemExit) as error:
                demo.main()
            self.assertEqual(error.exception.code, 2)
            self.assertIn('clean committed git tree', stderr.getvalue())
            storage.assert_not_called()
            run.assert_called_once_with(['git', 'status', '--porcelain'], cwd=demo.REPO,
                                        check=True, capture_output=True, text=True)


class StorageTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.storage = self.root / 'storage'
        self.repo = self.root / 'repo'
        self.job = self.storage / 'job'
        self.job.mkdir(parents=True)
        self.repo.mkdir()
        self.patches = patch.multiple(demo, STORAGE_ROOT=self.storage, REPO=self.repo)
        self.patches.start()
        self.addCleanup(self.patches.stop)

    def test_regular_nested_write(self):
        path = demo.safe_path(self.job, 'transactions/result.json')
        path.parent.mkdir()
        path.write_text('{}')
        demo.check_storage_tree(self.job, self.job)
        self.assertEqual(path.read_text(), '{}')

    def test_aliased_storage_root(self):
        alias = self.root / 'storage-alias'
        alias.symlink_to(self.storage, target_is_directory=True)
        with patch.object(demo, 'STORAGE_ROOT', alias):
            job = alias / 'job'
            path = demo.safe_path(job, 'result.json')
            self.assertEqual(path, self.job / 'result.json')
            path.write_text('{}')
            demo.check_storage_tree(job, job)
            for destination in [self.repo, self.root / 'outside']:
                with self.subTest(destination=destination):
                    link = job / 'escape'
                    link.symlink_to(destination, target_is_directory=True)
                    with self.assertRaises(RuntimeError):
                        demo.safe_path(job, 'escape/result.json')
                    with self.assertRaises(RuntimeError):
                        demo.check_storage_tree(job, job)
                    link.unlink()

    def test_job_symlink_outside_storage(self):
        alias = self.storage / 'alias'
        alias.symlink_to(self.root)
        with self.assertRaises(RuntimeError):
            demo.safe_path(alias)

    def test_job_symlink_into_repo(self):
        alias = self.storage / 'alias'
        alias.symlink_to(self.repo)
        with self.assertRaises(RuntimeError):
            demo.safe_path(alias)

    def test_directory_and_file_symlinks(self):
        for rel in ['transactions', 'credentials', 'logs', 'watcher', 'results.json',
                    'accounts.json', 'init-policy.json', 'owner-account.txt', 'faucet.log']:
            for destination in [self.repo, self.root / 'missing', self.storage / 'other-job']:
                with self.subTest(rel=rel, destination=destination):
                    link = self.job / rel
                    link.symlink_to(destination)
                    with self.assertRaises(RuntimeError):
                        demo.safe_path(self.job, rel)
                    with self.assertRaises(RuntimeError):
                        demo.check_storage_tree(self.job, self.job)
                    link.unlink()

    def test_nested_credential_symlink(self):
        credentials = self.job / 'credentials'
        (credentials / 'testnet').mkdir(parents=True)
        (credentials / 'testnet' / 'synthetic.json').symlink_to(self.repo / 'synthetic.json')
        with self.assertRaises(RuntimeError):
            demo.check_storage_tree(credentials)

    def test_traversal(self):
        with self.assertRaises(RuntimeError):
            demo.safe_path(self.job, '../escaped.json')

    def test_internal_symlink(self):
        (self.job / 'actual').mkdir()
        (self.job / 'alias').symlink_to(self.job / 'actual')
        self.assertEqual(demo.safe_path(self.job, 'alias/result.json'), self.job / 'actual/result.json')
        demo.check_storage_tree(self.job, self.job)

    def test_intermediate_symlink_cannot_leave_and_return(self):
        outside = self.root / 'outside'
        outside.mkdir()
        (outside / 'back').symlink_to(self.job)
        (self.job / 'escape').symlink_to(outside)
        with self.assertRaises(RuntimeError):
            demo.safe_path(self.job, 'escape/back/result.json')

    def test_flow_rejects_transactions_symlink_before_mkdir(self):
        (self.job / 'transactions').symlink_to(self.repo)
        with self.assertRaises(RuntimeError):
            Flow('synthetic.testnet', self.job, 'unused', self.job / 'credentials')
        self.assertFalse((self.job / 'logs').exists())


if __name__ == '__main__':
    unittest.main()
