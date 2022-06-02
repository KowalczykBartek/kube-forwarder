## how to build
if you want to build it from source, you need rust (super simple install)
```
https://www.rust-lang.org/tools/install
```
then just
```
sudo ./target/debug/kube-forwarder --kube-config /Users/kubeconfig.yaml forward-traffic 
```
to forward traffic to appropriate POD. Sudo is required because of http is running on port 80.
When forwarder is running, you can curl using kube-dns entries (curl -X GET http://your-app.namespace)

To generate entries for /etc/hosts, type 
```
./target/debug/kube-forwarder --kube-config /Users/kubeconfig.yaml generate-etc-hosts-entries namespace1 namespace2
```
/etc/hosts needs to be manually edited

## how it works
basically that is how it works
![howitworks](howitworks.png)
