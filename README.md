## how to build
if you want to build it from source, you need rust (super simple install)
```
https://www.rust-lang.org/tools/install
```
then just
```
sudo ./target/debug/kube-forwarder --kube-config /Users/kubeconfig.yaml
```
to forward traffic to appropriate POD. Sudo is required because of http is running on port 80.
When forwarder is running, you can curl using kube-dns entries (curl -X GET http://your-app.namespace)

To make kube-forwarder working, you need to add necessary entries in /etc/hosts. To handle following request:
```
curl http://test-app1.namespace1/some/resource
```
add 
```
127.0.0.1 test-app1.namespace1
```
in /etc/hosts

## mocks
during local developemt you may want to return mocked response instead requesting port-forwarder app, to do this you can 
provide file with mocks
```
--mock-location mocks.json
```
where json file has following format ([example file](overrides.json))

```json
[
    {
        "host": "application.test",
        "method": "GET",
        "match_uri_regex": "/test/[^/]*/example",
        "mocked_response": {
            "name": "John Smith",
            "sku": "20223",
            "price": 23.95
        },
        "headers": {
            "content-type": "application/json",
            "location": "http://application.test/api/something"
        },
        "status": 200
    }
]
```
then fowarder will match following request 
```
curl "http://application.test/test/abcd/example" | jq
{
  "sku": "20223",
  "price": 23.95,
  "name": "John Smith"
}
```
and respond with mocked body.

## cors
by passing
```
--apply-cors
```
you should be able to access port-forwarded app from browser.

## how it works
basically that is how it works
![howitworks](howitworks.png)
