# Deployments

Bosphorus instances deployed by `scripts/phase5-deploy.sh`. Programs are immutable: a new build means a new deployment.

## testnet (2026-10-20, commit e881180)

| What | Id |
|---|---|
| TKA token (collateral, 6 decimals) | `Public/2f3fofoDVYvep1V6J6awofLTiKomTdK8SVqc32kVTvVn` |
| TKB token (loan, 6 decimals) | `Public/o9pMMUqQpk3ufwdYXNje4TE9z8mQuB2VUPJY9nrcin5` |
| oracle | `Public/9x2KRzv2VK8BTm73rg6yzfjm3CKPmFvvzXjTNd6htJTs` |
| TKA/TKB feed (publisher `Public/7qJobryqwqQij19AEnkVzDL8qF9gwjpbWa49XnvmwKuS`) | `Public/GtEyQFa4xyDbRbns2HjSgsiQ5kuB8LmCRByUdSnhFh7U` |
| lending | `Public/8mnAx1h5Utbi1XCMUaHdACn6qM87Sek9NgKvdy2ewRKF` |
| TKB/TKA market (LLTV 80%, 2–112% APR kink at 80%, max price age 1 h) | `47dd288d6a29322c30d29644eeb83e1857e1d3dd96fc468138f02f0d28c34de9` |
| admin, fee recipient, mint authority | `Public/7qJobryqwqQij19AEnkVzDL8qF9gwjpbWa49XnvmwKuS` |

Image ids (`bos programs`; ELFs in `.testnet/elfs/`):

```
token 9226815c27c47bcac958dbaa4e50fde3dc12e01c5fb9b47874c2a165f238f734 343004 bytes
oracle 29cd0aa12b1710e12f7a126b568b36fe9da428b435abf1c5b889a69e6d31a512 349380 bytes
lending a995342c10373a3b0e004ae670b5ab281c2a6c944d25f413f3af580cdc91db7d 546160 bytes
```
